// EQ audio runtime state: tracks which LADSPA modules and loopbacks are active
// for each channel so the D-Bus interface can swap them when EQ is toggled.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use device_config::codec::FieldValue;

use crate::audio::{self, AudioSetup};
use crate::eq::ladspa;
use crate::eq::preset::{load_preset, preset_path, BandMode, EqBand, EqPreset};
use crate::eq::settings::{ChannelEqSettings, EqBackend};
use crate::state::{AppState, DeviceCommand};

/// Sink names for the EQ virtual sinks.
pub const MEDIA_EQ_SINK: &str = "Arctis_Media_EQ_internal";
pub const CHAT_EQ_SINK: &str = "Arctis_Chat_EQ_internal";

// ── Hardware EQ context ───────────────────────────────────────────────────────

/// Everything `apply_channel_eq` needs to drive hardware EQ on a device.
/// Constructed by the caller from `AppState`; `apply_channel_eq` stays agnostic
/// of `AppState` itself.
pub struct HwEqContext {
    /// Channel to the active device session.
    pub cmd_tx: mpsc::Sender<DeviceCommand>,
    /// Band mode the device hardware natively supports.
    pub native_band_mode: BandMode,
    /// Number of gain bands (determines field names `gain1`..`gainN`).
    pub num_bands: u8,
    /// Preset slot number to activate after writing gains (NovaPro: 18 = custom).
    pub custom_slot: u8,
    /// Whether `selected_eq_preset` API is available to commit the custom slot.
    pub has_preset_select: bool,
}

/// Synthesise a flat (all-zero) Fixed10 preset used when `preset_name` is empty or "Flat".
/// Fixed10 is chosen because it is compatible with the HW path on all current devices and
/// with the LADSPA mbeq_1197 pipeline; devices that use Parametric10 natively will
/// auto-fall-back to LADSPA via the existing band-mode mismatch check.
fn flat_preset() -> EqPreset {
    EqPreset {
        name: "Flat".to_string(),
        band_mode: BandMode::Fixed10,
        bands: vec![EqBand::gain_only(0.0); 10],
    }
}

/// Build a `HwEqContext` from the first connected device, or `None` if the
/// device has no hardware EQ API.
pub fn build_hw_eq_context(state: &AppState) -> Option<HwEqContext> {
    let entry = state.devices.values().next()?;
    let apis = entry.config.apis.as_ref()?;
    if !apis.contains_key("custom_eq") {
        return None;
    }
    Some(HwEqContext {
        cmd_tx: entry.cmd_tx.clone(),
        native_band_mode: BandMode::Fixed10,
        num_bands: 10,
        custom_slot: 4,
        has_preset_select: apis.contains_key("selected_eq_preset"),
    })
}

// ── Per-channel runtime ───────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ChannelEqState {
    /// LADSPA `module-ladspa-sink` module index (when loaded).
    ladspa_module_id: Option<u32>,
    /// Loopback module from `<Channel>.monitor → EQ sink` (when loaded).
    eq_loopback_id: Option<u32>,
    /// Whether the channel is currently routing through the EQ sink.
    active: bool,
}

// ── EQ runtime ────────────────────────────────────────────────────────────────

/// Shared state managed by the EQ D-Bus interface.
/// Wrapped in `Arc<Mutex<_>>` and passed to `EqInterface`.
#[derive(Debug, Default)]
pub struct EqRuntime {
    media: ChannelEqState,
    chat: ChannelEqState,
}

impl EqRuntime {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }
}

// ── Apply / tear-down ─────────────────────────────────────────────────────────

/// Outcome of [`apply_channel_eq`]: distinguishes the path taken so callers can
/// react differently (e.g. persist the hardware preset slot on success).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqApplyOutcome {
    /// EQ was applied via device hardware; the value is the custom preset slot.
    HwSlot(u8),
    /// EQ was applied via LADSPA software pipeline.
    Ladspa,
    /// Apply failed.
    Failed,
}

/// Apply (or update) the EQ on a single channel.
///
/// If LADSPA is not yet loaded for this channel:
///   1. Load a `module-ladspa-sink` (channel_eq_sink → physical).
///   2. Unload the direct channel loopback.
///   3. Load a new loopback: channel.monitor → channel_eq_sink.
///   4. Update `AudioSetup` loopback ID and `EqRuntime` state.
///
/// If LADSPA is already loaded, push new gains live (no reload).
pub async fn apply_channel_eq(
    ch_settings: &ChannelEqSettings,
    channel: &str, // "media" or "chat"
    base_dir: &std::path::Path,
    audio_shared: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&HwEqContext>,
) -> EqApplyOutcome {
    let preset_name = &ch_settings.preset;

    // Empty string or the special name "Flat" means no-effect EQ: synthesise flat gains
    // rather than trying to load a file (which would fail with a misleading error).
    let preset = if preset_name.is_empty() || preset_name.eq_ignore_ascii_case("flat") {
        flat_preset()
    } else {
        let preset_file = preset_path(base_dir, preset_name);
        match load_preset(&preset_file) {
            Ok(p) => p,
            Err(e) => {
                warn!("eq: cannot load preset '{preset_name}': {e}");
                return EqApplyOutcome::Failed;
            }
        }
    };

    // Hardware path: Auto resolves to hardware when a context is available;
    // explicit Hardware requires it.  Ladspa always bypasses hardware.
    let want_hw = !matches!(ch_settings.backend, EqBackend::Ladspa);
    if want_hw {
        if let Some(ctx) = hw_ctx {
            // Flat/empty preset in HW mode: select factory slot 0 (same as disable_channel_eq),
            // no custom slot write needed.
            if (preset_name.is_empty() || preset_name.eq_ignore_ascii_case("flat"))
                && ctx.has_preset_select
            {
                info!("eq: {channel} flat preset → selecting factory slot 0");
                let slot = HashMap::from([("eq_preset".to_string(), FieldValue::U8(0))]);
                let _ = ctx
                    .cmd_tx
                    .send(DeviceCommand::WriteApi {
                        api_name: "selected_eq_preset".to_string(),
                        values: slot,
                    })
                    .await;
                return EqApplyOutcome::HwSlot(0);
            }

            if preset.band_mode != ctx.native_band_mode {
                warn!(
                    "eq: preset band_mode {:?} != device native {:?} for {channel}; \
                     cannot use HW path",
                    preset.band_mode, ctx.native_band_mode
                );
                if matches!(ch_settings.backend, EqBackend::Hardware) {
                    return EqApplyOutcome::Failed; // hard failure for explicit hardware request
                }
                info!("eq: {channel} auto-fallback to LADSPA (band_mode mismatch)");
            } else {
                // Build gain fields: gain1..gainN clamped to device range ±10 dB.
                let n = ctx.num_bands as usize;
                let raw_gains: Vec<f32> = preset.bands.iter().take(n).map(|b| b.gain).collect();
                let clamped_gains: Vec<f32> =
                    raw_gains.iter().map(|&g| g.clamp(-10.0, 10.0)).collect();

                // Warn about any band that needed clamping.
                for (i, (&raw, &clamped)) in raw_gains.iter().zip(clamped_gains.iter()).enumerate()
                {
                    if (raw - clamped).abs() > 1e-4 {
                        warn!(
                            "eq: {channel} band {} gain clamped: {raw:.2} → {clamped:.2} dB",
                            i + 1
                        );
                    }
                }

                // Log the full gain vector being sent to the device.
                let gains_str: Vec<String> =
                    clamped_gains.iter().map(|g| format!("{g:.2}")).collect();
                info!(
                    "eq: HW EQ applying for {channel} (preset='{preset_name}', \
                     backend={:?}, bands=[{}])",
                    ch_settings.backend,
                    gains_str.join(", ")
                );

                let gains: HashMap<String, FieldValue> = clamped_gains
                    .iter()
                    .enumerate()
                    .map(|(i, &g)| (format!("gain{}", i + 1), FieldValue::F32(g)))
                    .collect();

                // Select the custom slot BEFORE writing gains: switching presets
                // reloads from flash, which would overwrite our gains if written first.
                // With the slot pre-selected, the 0x33 gain write lands in RAM and
                // takes effect immediately.
                if ctx.has_preset_select {
                    info!(
                        "eq: {channel} selecting custom slot {} on device",
                        ctx.custom_slot
                    );
                    let slot =
                        HashMap::from([("eq_preset".to_string(), FieldValue::U8(ctx.custom_slot))]);
                    if ctx
                        .cmd_tx
                        .send(DeviceCommand::WriteApi {
                            api_name: "selected_eq_preset".to_string(),
                            values: slot,
                        })
                        .await
                        .is_err()
                    {
                        warn!("eq: HW preset select failed for {channel}");
                        return EqApplyOutcome::Failed;
                    }
                }
                if ctx
                    .cmd_tx
                    .send(DeviceCommand::WriteApi {
                        api_name: "custom_eq".to_string(),
                        values: gains,
                    })
                    .await
                    .is_err()
                {
                    warn!("eq: HW EQ write failed for {channel}");
                    return EqApplyOutcome::Failed;
                }
                info!("eq: HW EQ applied for {channel} (preset='{preset_name}')");
                return EqApplyOutcome::HwSlot(ctx.custom_slot);
            }
        } else if matches!(ch_settings.backend, EqBackend::Hardware) {
            warn!("eq: hardware backend requested but no HW EQ context for {channel}");
            return EqApplyOutcome::Failed;
        }
    }

    let gains = ladspa::gains_for_preset(&preset);
    {
        let gains_str: Vec<String> = gains
            .iter()
            .enumerate()
            .map(|(i, g)| format!("band{}={g:.2}", i + 1))
            .collect();
        info!(
            "eq: LADSPA applying for {channel} (preset='{preset_name}', \
             backend={:?}, bands=[{}])",
            ch_settings.backend,
            gains_str.join(", ")
        );
    }
    let (eq_sink, source_sink) = if channel == "media" {
        (MEDIA_EQ_SINK, audio::MEDIA_SINK)
    } else {
        (CHAT_EQ_SINK, audio::CHAT_SINK)
    };

    // Get the physical sink name and current loopback ID.
    let (physical, direct_lb_id) = {
        let guard = audio_shared.lock().await;
        match guard.as_ref() {
            None => {
                warn!("eq: no audio setup; cannot apply EQ for {channel}");
                return EqApplyOutcome::Failed;
            }
            Some(s) => {
                let lb = if channel == "media" {
                    s.media_loopback
                } else {
                    s.chat_loopback
                };
                (s.physical_sink.clone(), lb)
            }
        }
    };

    // Check whether the LADSPA module is already live.
    let existing_ladspa_id = {
        let rt = eq_rt.lock().await;
        if channel == "media" {
            rt.media.ladspa_module_id
        } else {
            rt.chat.ladspa_module_id
        }
    };

    // pw-cli set-param returns Ok but gains don't update in practice; always
    // tear down and reload to guarantee the new values take effect.
    if let Some(id) = existing_ladspa_id {
        let _ = ladspa::unload_eq_module(id).await;
    }

    // Load LADSPA sink.
    let ladspa_id = match ladspa::load_eq_module(eq_sink, &physical, &gains).await {
        Ok(id) => id,
        Err(e) => {
            warn!("eq: failed to load LADSPA sink for {channel}: {e}");
            return EqApplyOutcome::Failed;
        }
    };

    // Remove the direct loopback (channel → physical) to avoid double playback.
    if let Err(e) = crate::audio::unload_module_by_id(direct_lb_id).await {
        warn!("eq: failed to unload direct loopback {direct_lb_id}: {e}");
    }

    // Create new loopback: channel.monitor → eq_sink.
    let source = format!("{source_sink}.monitor");
    let lb_args = format!("source={source} sink={eq_sink} latency_msec=0");
    let new_lb_id = match crate::audio::load_module_pub("module-loopback", &lb_args).await {
        Some(id) => id,
        None => {
            warn!("eq: failed to create EQ loopback for {channel}");
            // Attempt to restore the original loopback.
            let restore_args = format!("source={source} sink={physical} latency_msec=0");
            if let Some(id) = crate::audio::load_module_pub("module-loopback", &restore_args).await
            {
                let mut guard = audio_shared.lock().await;
                if let Some(s) = guard.as_mut() {
                    if channel == "media" {
                        s.media_loopback = id;
                    } else {
                        s.chat_loopback = id;
                    }
                }
            }
            return EqApplyOutcome::Failed;
        }
    };

    // Store new IDs.
    {
        let mut guard = audio_shared.lock().await;
        if let Some(s) = guard.as_mut() {
            if channel == "media" {
                s.media_loopback = new_lb_id;
            } else {
                s.chat_loopback = new_lb_id;
            }
        }
    }
    {
        let mut rt = eq_rt.lock().await;
        let ch = if channel == "media" {
            &mut rt.media
        } else {
            &mut rt.chat
        };
        ch.ladspa_module_id = Some(ladspa_id);
        ch.eq_loopback_id = Some(new_lb_id);
        ch.active = true;
    }

    info!("eq: LADSPA EQ enabled for {channel} (ladspa={ladspa_id}, loopback={new_lb_id})");
    EqApplyOutcome::Ladspa
}

/// Disable EQ on a channel: reset hardware EQ to flat (preset 0) if active,
/// remove the LADSPA sink and EQ loopback, restore the direct loopback.
pub async fn disable_channel_eq(
    channel: &str,
    audio_shared: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&HwEqContext>,
) {
    // Reset device EQ to the neutral preset slot (0 = flat/off).
    if let Some(ctx) = hw_ctx {
        if ctx.has_preset_select {
            let slot = HashMap::from([("eq_preset".to_string(), FieldValue::U8(0))]);
            let _ = ctx
                .cmd_tx
                .send(DeviceCommand::WriteApi {
                    api_name: "selected_eq_preset".to_string(),
                    values: slot,
                })
                .await;
        }
    }
    let (ladspa_id, eq_lb_id) = {
        let rt = eq_rt.lock().await;
        let ch = if channel == "media" {
            &rt.media
        } else {
            &rt.chat
        };
        (ch.ladspa_module_id, ch.eq_loopback_id)
    };

    let (physical, source_sink) = {
        let guard = audio_shared.lock().await;
        match guard.as_ref() {
            None => return,
            Some(s) => (
                s.physical_sink.clone(),
                if channel == "media" {
                    audio::MEDIA_SINK.to_owned()
                } else {
                    "Arctis_Chat".to_owned()
                },
            ),
        }
    };

    // Remove EQ loopback.
    if let Some(id) = eq_lb_id {
        if let Err(e) = crate::audio::unload_module_by_id(id).await {
            warn!("eq: failed to unload EQ loopback {id}: {e}");
        }
    }

    // Remove LADSPA sink.
    if let Some(id) = ladspa_id {
        let _ = ladspa::unload_eq_module(id).await;
    }

    // Only restore direct loopback when EQ was actually active. If neither
    // ladspa_id nor eq_lb_id was set the direct loopback is already in place;
    // creating another one would accumulate duplicate loopbacks.
    if ladspa_id.is_some() || eq_lb_id.is_some() {
        let source = format!("{source_sink}.monitor");
        let args = format!("source={source} sink={physical} latency_msec=0");
        let restored_lb_id = crate::audio::load_module_pub("module-loopback", &args).await;

        let mut guard = audio_shared.lock().await;
        if let Some(s) = guard.as_mut() {
            if let Some(id) = restored_lb_id {
                if channel == "media" {
                    s.media_loopback = id;
                } else {
                    s.chat_loopback = id;
                }
            }
        }
    }
    {
        let mut rt = eq_rt.lock().await;
        let ch = if channel == "media" {
            &mut rt.media
        } else {
            &mut rt.chat
        };
        ch.ladspa_module_id = None;
        ch.eq_loopback_id = None;
        ch.active = false;
    }

    info!("eq: EQ disabled for {channel}");
}
