// D-Bus service: three interfaces matching the v2 bus API, all on bus name
// `name.giacomofurlan.ArctisManager.Next`.  State is shared with device tasks
// via `Arc<Mutex<AppState>>`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::state::SignalEvent as SE;
use device_config::codec::FieldValue;
use device_config::{DeviceConfig, FieldDef, FieldOrRef, FieldType, StructDef};
use serde_json::{Map, Value as JsonValue};
use tokio::sync::{broadcast, Mutex};
use tracing::{error, warn};
use zbus::object_server::SignalEmitter;
use zbus::{connection, interface};

use crate::audio::AudioSetup;
use crate::device_persistence;
use crate::eq::preset::{list_presets, load_preset, preset_path, save_preset, EqPreset};
use crate::eq::settings::{load_eq_settings, save_eq_settings};
use crate::eq_manager::{self as eq_manager, EqRuntime};
use crate::general_settings::GeneralSettings;
use crate::state::{AppState, DeviceCommand, SignalEvent};

// ── D-Bus constants ───────────────────────────────────────────────────────────

const BUS_NAME: &str = "name.giacomofurlan.ArctisManager.Next";
const STATUS_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Status";
const SETTINGS_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Settings";
const CONFIG_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Config";
const EQ_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/EQ";
const NC_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/NC";

// ── Status interface ──────────────────────────────────────────────────────────

struct StatusInterface {
    state: Arc<Mutex<AppState>>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Status")]
impl StatusInterface {
    async fn get_status(&self) -> String {
        let state = self.state.lock().await;
        build_status_json(&state)
    }

    #[zbus(signal)]
    async fn status_changed(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_connected(
        emitter: &SignalEmitter<'_>,
        product_id: u16,
        name: &str,
        capabilities: Vec<String>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_disconnected(emitter: &SignalEmitter<'_>, product_id: u16) -> zbus::Result<()>;
}

// ── Settings interface ────────────────────────────────────────────────────────

struct SettingsInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
    settings_base_dir: std::path::PathBuf,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Settings")]
impl SettingsInterface {
    async fn get_settings(&self) -> String {
        let state = self.state.lock().await;
        build_settings_json(&state)
    }

    async fn set_setting(&self, setting: &str, value: &str) -> bool {
        // General settings are persisted to disk and do not require a connected device.
        if GeneralSettings::is_general_field(setting) {
            let (ok, path) = {
                let mut state = self.state.lock().await;
                let ok = state.general_settings.set_field(setting, value);
                (ok, state.general_settings_path.clone())
            };
            if !ok {
                warn!("SetSetting: could not apply general field '{setting}' = '{value}'");
                return false;
            }
            if let Err(e) = {
                let state = self.state.lock().await;
                state.general_settings.save_to_file(&path)
            } {
                warn!("SetSetting: failed to persist general settings: {e}");
            }
            let json = {
                let s = self.state.lock().await;
                build_settings_json(&s)
            };
            let _ = self.signal_tx.send(SE::SettingsChanged { json });
            return true;
        }

        // Device settings require a connected device and a matching API.
        let (api_name, field_value) = {
            let state = self.state.lock().await;
            let Some(entry) = state.devices.values().find(|e| !e.status.is_empty()) else {
                return false;
            };
            let Some(api) = find_api_for_field(&entry.config, setting) else {
                warn!("SetSetting: no API writes field '{setting}'");
                return false;
            };
            let fv = match parse_setting_value(&entry.config, &api, setting, value) {
                Some(fv) => fv,
                None => {
                    warn!("SetSetting: could not parse value '{value}' for '{setting}'");
                    return false;
                }
            };
            (api, fv)
        };

        let sent = {
            let state = self.state.lock().await;
            let Some(entry) = state.devices.values().find(|e| !e.status.is_empty()) else {
                return false;
            };
            // For multi-field structs, populate sibling fields from current status.
            let values = build_write_values(
                &entry.config,
                &api_name,
                setting,
                field_value,
                &entry.status,
            );
            entry
                .cmd_tx
                .send(DeviceCommand::WriteApi { api_name, values })
                .await
                .is_ok()
        };

        if sent {
            let json_val: serde_json::Value =
                serde_json::from_str(value).unwrap_or(serde_json::Value::Null);

            // Persist the written value: load existing overrides, update, save.
            let (vid, pid) = {
                let s = self.state.lock().await;
                if let Some(entry) = s.devices.values().find(|e| !e.status.is_empty()) {
                    (entry.vid, entry.pid)
                } else {
                    (0, 0)
                }
            };
            if vid != 0 || pid != 0 {
                let file_path =
                    device_persistence::settings_file_path(&self.settings_base_dir, vid, pid);
                let mut overrides = device_persistence::load_device_settings(&file_path);
                overrides.insert(setting.to_string(), json_val.clone());
                if let Err(e) = device_persistence::save_device_settings(&file_path, &overrides) {
                    warn!("SetSetting: failed to persist device settings: {e}");
                }
            }

            // Optimistic update: reflect the new value in the status map so that
            // the SettingsChanged signal carries the new value rather than the
            // stale one from the last sync read.  This prevents the GUI from
            // reverting the widget to the old value on receipt of SettingsChanged.
            {
                let mut s = self.state.lock().await;
                if let Some(entry) = s.devices.values_mut().next() {
                    let slot = entry
                        .status
                        .entry(setting.to_string())
                        .or_insert_with(|| serde_json::json!({"value": null, "type": null}));
                    if let Some(v) = slot.get_mut("value") {
                        *v = json_val;
                    }
                }
            }

            let json = {
                let s = self.state.lock().await;
                build_settings_json(&s)
            };
            let _ = self.signal_tx.send(SE::SettingsChanged { json });
        }

        sent
    }

    async fn get_version(&self) -> String {
        env!("LAM_VERSION").to_string()
    }

    async fn get_list_options(&self, list_name: &str) -> String {
        match list_name {
            "pulse_audio_devices" => {
                let sinks = crate::audio::list_audio_sinks().await;
                serde_json::to_string(&sinks).unwrap_or_else(|_| "[]".to_string())
            }
            _ => "[]".to_string(),
        }
    }

    #[zbus(signal)]
    async fn settings_changed(emitter: &SignalEmitter<'_>, settings_json: &str)
        -> zbus::Result<()>;
}

// ── EQ interface ──────────────────────────────────────────────────────────────

struct EqInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
    settings_base_dir: std::path::PathBuf,
    eq_runtime: Arc<Mutex<EqRuntime>>,
    audio_shared: Arc<Mutex<Option<AudioSetup>>>,
    focus_backend: crate::focus_monitor::FocusBackend,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.EQ")]
impl EqInterface {
    /// Returns EQ capabilities of the connected device.
    /// JSON: `{"has_hw_eq": bool, "hw_band_mode": "fixed_10"|"parametric_10"|"fixed_5"|null}`.
    async fn get_eq_capabilities(&self) -> String {
        use crate::eq::preset::BandMode;
        use crate::eq::resample;
        let ladspa_available = crate::eq::ladspa::check_plugin_available().await;
        let state = self.state.lock().await;
        let (has_hw_eq, hw_band_mode): (bool, Option<&str>) = state
            .devices
            .values()
            .find_map(|entry| {
                let apis = entry.config.apis.as_ref()?;
                if apis.contains_key("custom_eq") {
                    Some((true, Some("fixed_10")))
                } else {
                    None
                }
            })
            .unwrap_or((false, None));
        let hw_freqs: Vec<f32> = if has_hw_eq {
            resample::hw_freqs_for_mode(BandMode::Fixed10).to_vec()
        } else {
            vec![]
        };
        let fb = &self.focus_backend;
        serde_json::to_string(&serde_json::json!({
            "ladspa_available": ladspa_available,
            "has_hw_eq": has_hw_eq,
            "hw_band_mode": hw_band_mode,
            "hw_freqs": hw_freqs,
            "hw_override_backend": crate::focus_monitor::backend_id(fb),
            "hw_override_unsupported_reason": crate::focus_monitor::unsupported_reason(fb),
        }))
        .unwrap_or_else(|_| r#"{"ladspa_available":false,"has_hw_eq":false,"hw_band_mode":null,"hw_freqs":[],"hw_override_backend":"unsupported","hw_override_unsupported_reason":null}"#.to_string())
    }

    /// Resample a named software EQ preset to the hardware custom EQ slot and activate it.
    /// Returns an empty string on success, or an error message on failure.
    async fn apply_hw_preset(&self, preset_name: String) -> zbus::fdo::Result<String> {
        use crate::eq::resample;
        use crate::state::DeviceCommand;

        let preset = load_preset(&preset_path(&self.settings_base_dir, &preset_name))
            .map_err(zbus::fdo::Error::Failed)?;

        let cmd_tx = {
            let state = self.state.lock().await;
            let entry = state
                .devices
                .values()
                .find(|e| {
                    e.config
                        .apis
                        .as_ref()
                        .map(|a| a.contains_key("custom_eq"))
                        .unwrap_or(false)
                })
                .ok_or_else(|| {
                    zbus::fdo::Error::Failed("no device with hardware EQ connected".into())
                })?;
            entry.cmd_tx.clone()
        };

        let gains = resample::resample(&preset, &resample::FIXED_10_HZ, (-10.0, 10.0));

        let eq_values: HashMap<String, FieldValue> = (1..=10)
            .map(|i| (format!("gain{i}"), FieldValue::F32(gains[i - 1])))
            .collect();
        cmd_tx
            .send(DeviceCommand::WriteApi {
                api_name: "custom_eq".into(),
                values: eq_values,
            })
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        let mut preset_values = HashMap::new();
        preset_values.insert("eq_preset".to_string(), FieldValue::U8(4));
        cmd_tx
            .send(DeviceCommand::WriteApi {
                api_name: "selected_eq_preset".into(),
                values: preset_values,
            })
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        // Persist eq_preset=4 (custom slot) and emit SettingsChanged so the Device
        // tab QComboBox reflects the custom slot without requiring a tab re-visit.
        let (vid, pid) = {
            let s = self.state.lock().await;
            s.devices
                .values()
                .next()
                .map(|e| (e.vid, e.pid))
                .unwrap_or((0, 0))
        };
        if vid != 0 || pid != 0 {
            let file_path =
                device_persistence::settings_file_path(&self.settings_base_dir, vid, pid);
            let mut overrides = device_persistence::load_device_settings(&file_path);
            overrides.insert("eq_preset".to_string(), serde_json::json!(4u8));
            if let Err(e) = device_persistence::save_device_settings(&file_path, &overrides) {
                warn!("apply_hw_preset: failed to persist eq_preset: {e}");
            }
        }
        {
            let mut s = self.state.lock().await;
            if let Some(entry) = s.devices.values_mut().next() {
                let slot = entry
                    .status
                    .entry("eq_preset".to_string())
                    .or_insert_with(|| serde_json::json!({"value": null, "type": null}));
                if let Some(v) = slot.get_mut("value") {
                    *v = serde_json::json!(4u8);
                }
            }
        }
        let json = {
            let s = self.state.lock().await;
            build_settings_json(&s)
        };
        let _ = self.signal_tx.send(SE::SettingsChanged { json });

        Ok(String::new())
    }

    /// Persists `eq_preset = slot` to device settings on disk, updates in-memory
    /// status, and emits `SettingsChanged` so the GUI updates the active-preset label.
    async fn persist_eq_preset_slot(&self, slot: u8) {
        let (vid, pid) = {
            let s = self.state.lock().await;
            s.devices
                .values()
                .next()
                .map(|e| (e.vid, e.pid))
                .unwrap_or((0, 0))
        };
        if vid != 0 || pid != 0 {
            let file_path =
                device_persistence::settings_file_path(&self.settings_base_dir, vid, pid);
            let mut overrides = device_persistence::load_device_settings(&file_path);
            overrides.insert("eq_preset".to_string(), serde_json::json!(slot));
            if let Err(e) = device_persistence::save_device_settings(&file_path, &overrides) {
                warn!("eq: failed to persist eq_preset slot {slot}: {e}");
            }
        }
        {
            let mut s = self.state.lock().await;
            if let Some(entry) = s.devices.values_mut().next() {
                let field = entry
                    .status
                    .entry("eq_preset".to_string())
                    .or_insert_with(|| serde_json::json!({"value": null, "type": null}));
                if let Some(v) = field.get_mut("value") {
                    *v = serde_json::json!(slot);
                }
            }
        }
        let json = {
            let s = self.state.lock().await;
            build_settings_json(&s)
        };
        let _ = self.signal_tx.send(SE::SettingsChanged { json });
    }

    /// Returns the full EQ settings JSON (both channels).
    async fn get_eq_settings(&self) -> String {
        let settings = load_eq_settings(&self.settings_base_dir);
        serde_json::to_string(&settings).unwrap_or_else(|_| "{}".to_string())
    }

    /// Set a single EQ field on a channel.
    ///
    /// `channel`: `"media"` or `"chat"`.
    /// `key`: `"enabled"`, `"backend"`, `"band_mode"`, or `"preset"`.
    /// `value`: JSON-encoded value.
    async fn set_eq_setting(&self, channel: &str, key: &str, value: &str) -> bool {
        if channel != "media" && channel != "chat" {
            return false;
        }
        let mut settings = load_eq_settings(&self.settings_base_dir);
        let ch = if channel == "media" {
            &mut settings.media
        } else {
            &mut settings.chat
        };

        let ok = match key {
            "enabled" => {
                if let Ok(b) = serde_json::from_str::<bool>(value) {
                    ch.enabled = b;
                    true
                } else {
                    false
                }
            }
            "backend" => {
                if let Ok(b) = serde_json::from_str(value) {
                    ch.backend = b;
                    true
                } else {
                    false
                }
            }
            "band_mode" => {
                if let Ok(m) = serde_json::from_str(value) {
                    ch.band_mode = m;
                    true
                } else {
                    false
                }
            }
            "preset" => {
                if let Ok(name) = serde_json::from_str::<String>(value) {
                    ch.preset = name;
                    true
                } else {
                    false
                }
            }
            "app_overrides" => {
                use crate::eq::settings::AppOverride;
                if let Ok(overrides) = serde_json::from_str::<Vec<AppOverride>>(value) {
                    ch.app_overrides = overrides;
                    true
                } else {
                    warn!("SetEQSetting: invalid app_overrides JSON");
                    false
                }
            }
            _ => false,
        };

        if !ok {
            return false;
        }

        if let Err(e) = save_eq_settings(&self.settings_base_dir, &settings) {
            warn!("SetEQSetting: failed to persist: {e}");
        }

        // Apply or disable EQ pipeline.
        let ch_settings = if channel == "media" {
            &settings.media
        } else {
            &settings.chat
        }
        .clone();
        let hw_ctx = {
            let st = self.state.lock().await;
            eq_manager::build_hw_eq_context(&st)
        };
        if ch_settings.enabled {
            let outcome = eq_manager::apply_channel_eq(
                &ch_settings,
                channel,
                &self.settings_base_dir,
                &self.audio_shared,
                &self.eq_runtime,
                hw_ctx.as_ref(),
            )
            .await;
            if let eq_manager::EqApplyOutcome::HwSlot(slot) = outcome {
                self.persist_eq_preset_slot(slot).await;
            }
        } else {
            eq_manager::disable_channel_eq(
                channel,
                &self.audio_shared,
                &self.eq_runtime,
                hw_ctx.as_ref(),
            )
            .await;
        }

        let json = serde_json::to_string(&settings).unwrap_or_default();
        let _ = self.signal_tx.send(SignalEvent::EQChanged { json });
        true
    }

    /// Atomically replace all settings for one channel and apply once.
    ///
    /// `channel`: `"media"` or `"chat"`.
    /// `json`: full `ChannelEqSettings` object (enabled, backend, band_mode, preset, app_overrides).
    async fn set_eq_channel_settings(&self, channel: &str, json: &str) -> bool {
        if channel != "media" && channel != "chat" {
            return false;
        }
        let ch_settings: crate::eq::settings::ChannelEqSettings = match serde_json::from_str(json) {
            Ok(s) => s,
            Err(e) => {
                warn!("SetEqChannelSettings: invalid JSON: {e}");
                return false;
            }
        };

        let mut settings = load_eq_settings(&self.settings_base_dir);
        if channel == "media" {
            settings.media = ch_settings.clone();
        } else {
            settings.chat = ch_settings.clone();
        }

        if let Err(e) = save_eq_settings(&self.settings_base_dir, &settings) {
            warn!("SetEqChannelSettings: failed to persist: {e}");
        }

        let hw_ctx = {
            let st = self.state.lock().await;
            eq_manager::build_hw_eq_context(&st)
        };
        if ch_settings.enabled {
            let outcome = eq_manager::apply_channel_eq(
                &ch_settings,
                channel,
                &self.settings_base_dir,
                &self.audio_shared,
                &self.eq_runtime,
                hw_ctx.as_ref(),
            )
            .await;
            if let eq_manager::EqApplyOutcome::HwSlot(slot) = outcome {
                self.persist_eq_preset_slot(slot).await;
            }
        } else {
            eq_manager::disable_channel_eq(
                channel,
                &self.audio_shared,
                &self.eq_runtime,
                hw_ctx.as_ref(),
            )
            .await;
        }

        let json = serde_json::to_string(&settings).unwrap_or_default();
        let _ = self.signal_tx.send(SignalEvent::EQChanged { json });
        true
    }

    /// Returns a JSON array of preset summaries: `[{"name": "...", "band_mode": "..."}]`.
    async fn list_presets(&self) -> String {
        let presets = list_presets(&self.settings_base_dir);
        let summaries: Vec<serde_json::Value> = presets
            .iter()
            .map(|p| serde_json::json!({"name": p.name, "band_mode": p.band_mode}))
            .collect();
        serde_json::to_string(&summaries).unwrap_or_else(|_| "[]".to_string())
    }

    /// Returns the full preset JSON for `name`, or `{}` if not found.
    async fn get_preset(&self, name: &str) -> String {
        let path = preset_path(&self.settings_base_dir, name);
        match load_preset(&path) {
            Ok(p) => serde_json::to_string(&p).unwrap_or_else(|_| "{}".to_string()),
            Err(_) => "{}".to_string(),
        }
    }

    /// Save (or overwrite) a preset from JSON.  Returns `true` on success.
    async fn save_preset(&self, preset_json: &str) -> bool {
        let preset: EqPreset = match serde_json::from_str(preset_json) {
            Ok(p) => p,
            Err(e) => {
                warn!("SavePreset: invalid JSON: {e}");
                return false;
            }
        };
        if let Err(e) = preset.validate() {
            warn!("SavePreset: invalid preset: {e}");
            return false;
        }
        match save_preset(&self.settings_base_dir, &preset) {
            Ok(()) => true,
            Err(e) => {
                warn!("SavePreset: save failed: {e}");
                false
            }
        }
    }

    /// Delete a preset by name.  Returns `true` if the file was removed.
    async fn delete_preset(&self, name: &str) -> bool {
        let path = preset_path(&self.settings_base_dir, name);
        match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) => {
                warn!("DeletePreset: {e}");
                false
            }
        }
    }

    /// Returns JSON array of currently active PulseAudio clients.
    /// Each entry: `{"name": "...", "pid": <u32>}`.
    async fn get_running_streams(&self) -> String {
        running_streams_json().await
    }

    /// Returns JSON array of installed Steam games.
    /// Each entry: `{"app_id": <u32>, "name": "..."}`.
    async fn get_steam_games(&self) -> String {
        steam_games_json()
    }

    #[zbus(signal)]
    async fn eq_changed(emitter: &SignalEmitter<'_>, settings_json: &str) -> zbus::Result<()>;
}

// ── EQ helpers ────────────────────────────────────────────────────────────────

fn is_system_stream(name: &str) -> bool {
    name.starts_with("pipewire")
        || name.starts_with("PulseAudio")
        || name.starts_with("WirePlumber")
        || matches!(
            name,
            "uresourced"
                | "kwin_wayland"
                | "KWin"
                | "libcanberra"
                | "xdg-desktop-portal"
                | "pactl"
                | "pulseaudio"
        )
}

async fn running_streams_json() -> String {
    let out = tokio::process::Command::new("pactl")
        .args(["-f", "json", "list", "clients"])
        .output()
        .await;
    let Ok(out) = out else {
        return "[]".to_string();
    };
    if !out.status.success() {
        return "[]".to_string();
    }
    let Ok(clients) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return "[]".to_string();
    };
    let Some(arr) = clients.as_array() else {
        return "[]".to_string();
    };
    let result: Vec<serde_json::Value> = arr
        .iter()
        .filter_map(|c| {
            let name = c["properties"]["application.name"].as_str()?.to_owned();
            if is_system_stream(&name) {
                return None;
            }
            let pid = c["properties"]["application.process.id"]
                .as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            Some(serde_json::json!({"name": name, "pid": pid}))
        })
        .collect();
    serde_json::to_string(&result).unwrap_or_else(|_| "[]".to_string())
}

fn steam_games_json() -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let Some(home) = home else {
        return "[]".to_string();
    };
    let steamapps = home.join(".steam/steam/steamapps");
    let Ok(entries) = std::fs::read_dir(&steamapps) else {
        return "[]".to_string();
    };
    let mut games: Vec<serde_json::Value> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "acf") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Parse minimal VDF: look for "appid" and "name" lines.
        let app_id: Option<u32> = content
            .lines()
            .find(|l| l.trim_start().starts_with(r#""appid""#))
            .and_then(|l| l.split('"').nth(3))
            .and_then(|s| s.parse().ok());
        let name: Option<&str> = content
            .lines()
            .find(|l| l.trim_start().starts_with(r#""name""#))
            .and_then(|l| l.split('"').nth(3));
        if let (Some(id), Some(n)) = (app_id, name) {
            games.push(serde_json::json!({"app_id": id, "name": n}));
        }
    }
    games.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    serde_json::to_string(&games).unwrap_or_else(|_| "[]".to_string())
}

// ── Config interface ──────────────────────────────────────────────────────────

struct ConfigInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Config")]
impl ConfigInterface {
    async fn reload_configs(&self) -> bool {
        let dirs = {
            let state = self.state.lock().await;
            state.config_dirs.clone()
        };
        let dir_refs: Vec<&std::path::Path> =
            dirs.iter().map(std::path::PathBuf::as_path).collect();
        let new_configs = crate::load_configs_from_dirs(&dir_refs);
        let json = {
            let mut state = self.state.lock().await;
            state.configs = new_configs;
            build_settings_json(&state)
        };
        let _ = self.signal_tx.send(SE::SettingsChanged { json });
        true
    }
}

// ── NC interface ──────────────────────────────────────────────────────────────

use crate::mic_router::MicRouterState;
use crate::nc_config::NcConfig;
use crate::nc_manager::NcRuntime;

fn nc_config_path(base: &std::path::Path) -> std::path::PathBuf {
    base.join("nc_config.json")
}

fn load_nc_config(base: &std::path::Path) -> NcConfig {
    std::fs::read_to_string(nc_config_path(base))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_nc_config(base: &std::path::Path, cfg: &NcConfig) -> bool {
    let path = nc_config_path(base);
    let Ok(json) = serde_json::to_string_pretty(cfg) else {
        return false;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, json).is_ok()
}

struct NcInterface {
    settings_base_dir: std::path::PathBuf,
    signal_tx: broadcast::Sender<SignalEvent>,
    nc_runtime: Arc<Mutex<NcRuntime>>,
    mic_router: Arc<Mutex<MicRouterState>>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.NC")]
impl NcInterface {
    #[zbus(name = "GetNCCapabilities")]
    async fn get_nc_capabilities(&self) -> String {
        let rnnoise = crate::nc_manager::rnnoise_plugin().is_some();
        let swh = crate::nc_manager::swh_available();
        serde_json::json!({
            "rnnoise_available": rnnoise,
            "swh_available": swh,
        })
        .to_string()
    }

    #[zbus(name = "GetNCSettings")]
    async fn get_nc_settings(&self) -> String {
        let cfg = load_nc_config(&self.settings_base_dir);
        serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string())
    }

    #[zbus(name = "SetNCSettings")]
    async fn set_nc_settings(&self, json: &str) -> bool {
        let cfg: NcConfig = match serde_json::from_str(json) {
            Ok(c) => c,
            Err(e) => {
                warn!("SetNcSettings: invalid JSON: {e}");
                return false;
            }
        };

        if !save_nc_config(&self.settings_base_dir, &cfg) {
            warn!("SetNcSettings: failed to persist config");
        }

        let output_source = {
            let mut rt = self.nc_runtime.lock().await;
            crate::nc_manager::apply_nc(&cfg, &mut rt).await
        };

        {
            let mut mr = self.mic_router.lock().await;
            match output_source {
                Some(src) => {
                    crate::mic_router::update(&mut mr, src).await;
                }
                None => {
                    crate::mic_router::teardown(&mut mr).await;
                }
            }
        }

        let out_json = serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string());
        let _ = self.signal_tx.send(SE::NCChanged { json: out_json });
        true
    }

    #[zbus(signal)]
    async fn nc_changed(emitter: &SignalEmitter<'_>, config_json: &str) -> zbus::Result<()>;
}

// ── Service startup ───────────────────────────────────────────────────────────

/// Register all interfaces on the session bus and spawn a background task that
/// forwards `SignalEvent`s from device tasks to D-Bus signals.
///
/// Returns the `Connection` so the caller can keep it alive.
pub async fn start_dbus_service(
    state: Arc<Mutex<AppState>>,
    mut signal_rx: broadcast::Receiver<SignalEvent>,
    signal_tx: broadcast::Sender<SignalEvent>,
    settings_base_dir: std::path::PathBuf,
    audio_shared: Arc<Mutex<Option<AudioSetup>>>,
    nc_runtime: Arc<Mutex<NcRuntime>>,
    mic_router: Arc<Mutex<MicRouterState>>,
) -> zbus::Result<zbus::Connection> {
    let eq_runtime = EqRuntime::new();
    let focus_backend = crate::focus_monitor::detect();

    // Clones for background monitors (spawned after conn is built).
    let monitor_audio = Arc::clone(&audio_shared);
    let monitor_eq_rt = Arc::clone(&eq_runtime);
    let monitor_base = settings_base_dir.clone();
    let monitor_state = Arc::clone(&state);
    let monitor_rx = signal_tx.subscribe();
    let focus_audio = Arc::clone(&audio_shared);
    let focus_eq_rt = Arc::clone(&eq_runtime);
    let focus_base = settings_base_dir.clone();
    let focus_state = Arc::clone(&state);
    let focus_rx = signal_tx.subscribe();

    let conn = connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(
            STATUS_PATH,
            StatusInterface {
                state: Arc::clone(&state),
            },
        )?
        .serve_at(
            SETTINGS_PATH,
            SettingsInterface {
                state: Arc::clone(&state),
                signal_tx: signal_tx.clone(),
                settings_base_dir: settings_base_dir.clone(),
            },
        )?
        .serve_at(
            CONFIG_PATH,
            ConfigInterface {
                state: Arc::clone(&state),
                signal_tx: signal_tx.clone(),
            },
        )?
        .serve_at(
            EQ_PATH,
            EqInterface {
                state: Arc::clone(&state),
                signal_tx: signal_tx.clone(),
                settings_base_dir: settings_base_dir.clone(),
                eq_runtime: Arc::clone(&eq_runtime),
                audio_shared,
                focus_backend: focus_backend.clone(),
            },
        )?
        .serve_at(
            NC_PATH,
            NcInterface {
                settings_base_dir,
                signal_tx: signal_tx.clone(),
                nc_runtime,
                mic_router,
            },
        )?
        .build()
        .await?;

    let status_emitter = SignalEmitter::new(&conn, STATUS_PATH)?.into_owned();
    let settings_emitter = SignalEmitter::new(&conn, SETTINGS_PATH)?.into_owned();
    let eq_emitter = SignalEmitter::new(&conn, EQ_PATH)?.into_owned();
    let nc_emitter = SignalEmitter::new(&conn, NC_PATH)?.into_owned();

    tokio::spawn(crate::stream_monitor::run(
        monitor_base,
        monitor_audio,
        monitor_eq_rt,
        monitor_state,
        monitor_rx,
    ));
    tokio::spawn(crate::focus_monitor::run(
        focus_base,
        focus_audio,
        focus_eq_rt,
        focus_state,
        focus_rx,
    ));

    tokio::spawn(async move {
        loop {
            let event = match signal_rx.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("D-Bus signal emitter lagged, dropped {n} events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            match event {
                SignalEvent::StatusChanged => {
                    let json = {
                        let s = state.lock().await;
                        build_status_json(&s)
                    };
                    if let Err(e) = StatusInterface::status_changed(&status_emitter, &json).await {
                        error!("StatusChanged signal failed: {e}");
                    }
                }
                SignalEvent::SettingsChanged { json } => {
                    if let Err(e) =
                        SettingsInterface::settings_changed(&settings_emitter, &json).await
                    {
                        error!("SettingsChanged signal failed: {e}");
                    }
                }
                SignalEvent::EQChanged { json } => {
                    if let Err(e) = EqInterface::eq_changed(&eq_emitter, &json).await {
                        error!("EQChanged signal failed: {e}");
                    }
                }
                SignalEvent::NCChanged { json } => {
                    if let Err(e) = NcInterface::nc_changed(&nc_emitter, &json).await {
                        error!("NCChanged signal failed: {e}");
                    }
                }
                SignalEvent::DeviceConnected {
                    pid,
                    name,
                    capabilities,
                } => {
                    // Push a SettingsChanged so the Device settings panel populates
                    // (device entry is already in state; status may be empty yet,
                    //  but the schema is available from config.apis).
                    let settings_json = {
                        let s = state.lock().await;
                        build_settings_json(&s)
                    };
                    if let Err(e) =
                        SettingsInterface::settings_changed(&settings_emitter, &settings_json).await
                    {
                        error!("SettingsChanged (on connect) signal failed: {e}");
                    }
                    if let Err(e) =
                        StatusInterface::device_connected(&status_emitter, pid, &name, capabilities)
                            .await
                    {
                        error!("DeviceConnected signal failed: {e}");
                    }
                }
                SignalEvent::DeviceDisconnected { pid } => {
                    // Device was removed from state before this event was sent, so
                    // build_status_json returns "{}" and build_settings_json returns
                    // general-only settings — both clear the GUI automatically.
                    let status_json = {
                        let s = state.lock().await;
                        build_status_json(&s)
                    };
                    if let Err(e) =
                        StatusInterface::status_changed(&status_emitter, &status_json).await
                    {
                        error!("StatusChanged (on disconnect) signal failed: {e}");
                    }
                    let settings_json = {
                        let s = state.lock().await;
                        build_settings_json(&s)
                    };
                    if let Err(e) =
                        SettingsInterface::settings_changed(&settings_emitter, &settings_json).await
                    {
                        error!("SettingsChanged (on disconnect) signal failed: {e}");
                    }
                    if let Err(e) = StatusInterface::device_disconnected(&status_emitter, pid).await
                    {
                        error!("DeviceDisconnected signal failed: {e}");
                    }
                }
            }
        }
    });

    Ok(conn)
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn build_status_json(state: &AppState) -> String {
    // Skip entries whose status map is empty: they are placeholder DeviceEntries
    // registered by a task that was aborted before device_init completed.
    let Some(entry) = state.devices.values().find(|e| !e.status.is_empty()) else {
        return "{}".to_string();
    };

    if let Some(representation) = &entry.config.representation {
        let mut result: Map<String, JsonValue> = Map::new();
        for (category, field_names) in representation {
            let mut cat_map: Map<String, JsonValue> = Map::new();
            for field_name in field_names {
                if let Some(val) = entry.status.get(field_name) {
                    cat_map.insert(field_name.clone(), val.clone());
                }
            }
            if !cat_map.is_empty() {
                result.insert(category.clone(), JsonValue::Object(cat_map));
            }
        }
        if result.is_empty() {
            return "{}".to_string();
        }
        return serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());
    }

    // Fallback: group all fields under "headset" when no representation is defined.
    let fields: Map<String, JsonValue> = entry
        .status
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let result = serde_json::json!({"headset": fields});
    serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string())
}

pub(crate) fn build_settings_json(state: &AppState) -> String {
    // Start with the general section, always present regardless of device connection.
    let general_json = state.general_settings.to_json();
    let general_config = GeneralSettings::settings_config_json();

    let mut result = serde_json::json!({
        "general": general_json,
        "device": {},
        "settings_config": general_config
    });

    let Some(entry) = state.devices.values().next() else {
        return serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
    };

    let (Some(apis), Some(structs)) = (&entry.config.apis, &entry.config.structs) else {
        return serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
    };

    let mut device_map = Map::new();
    let config_map = result["settings_config"].as_object_mut().unwrap();

    for (api_name, api_def) in apis {
        if api_def.write.is_none() {
            continue;
        }
        let Some(struct_def) = structs.get(api_name.as_str()) else {
            continue;
        };
        for fdef in outgoing_fields(struct_def, structs) {
            if fdef.constant.is_some() {
                continue;
            }
            let current = entry
                .status
                .get(&fdef.name)
                .and_then(|v| v.get("value"))
                .cloned();

            // device section: plain current value (no {value, type} wrapper)
            if let Some(val) = &current {
                device_map
                    .entry(fdef.name.clone())
                    .or_insert_with(|| val.clone());
            }

            config_map
                .entry(fdef.name.clone())
                .or_insert_with(|| field_to_schema(fdef, current.as_ref()));
        }
    }

    result["device"] = JsonValue::Object(device_map);

    serde_json::to_string(&result).unwrap_or_else(|_| result.to_string())
}

fn yaml_to_json(y: &serde_yaml::Value) -> JsonValue {
    if let Some(i) = y.as_i64() {
        JsonValue::from(i)
    } else if let Some(f) = y.as_f64() {
        JsonValue::from(f)
    } else if let Some(s) = y.as_str() {
        JsonValue::from(s)
    } else if let Some(b) = y.as_bool() {
        JsonValue::from(b)
    } else {
        JsonValue::Null
    }
}

/// Build a `ConfigSetting`-compatible schema for one writable field.
///
/// The GUI constructs `ConfigSetting(name=..., **schema)`, so every key here
/// becomes an attribute.  Required keys: `type` (SettingType value string) and
/// `default_value`.
fn field_to_schema(fdef: &FieldDef, current_val: Option<&JsonValue>) -> JsonValue {
    // default_value: prefer the live current value, fall back to range minimum.
    let default_val = current_val.cloned().unwrap_or_else(|| {
        fdef.range
            .as_ref()
            .and_then(|r| r.first())
            .map(yaml_to_json)
            .unwrap_or(JsonValue::from(0i64))
    });

    // Explicit values_mapping → discrete_map regardless of range.
    if let Some(vm) = &fdef.values_mapping {
        let mapping: Map<String, JsonValue> = vm
            .iter()
            .map(|(k, v)| (k.clone(), JsonValue::from(v.as_str())))
            .collect();
        return serde_json::json!({
            "type": "discrete_map",
            "default_value": default_val,
            "values_mapping": JsonValue::Object(mapping)
        });
    }

    if let Some(range) = &fdef.range {
        if range.len() == 2 {
            let min_f = range[0].as_f64().unwrap_or(0.0);
            let max_f = range[1].as_f64().unwrap_or(1.0);
            if (max_f - min_f) <= 1.0 {
                // Boolean [0, 1] range → toggle
                return serde_json::json!({
                    "type": "toggle",
                    "default_value": default_val,
                    "values": {
                        "on": yaml_to_json(&range[1]),
                        "off": yaml_to_json(&range[0]),
                        "on_label": "on",
                        "off_label": "off"
                    }
                });
            } else {
                // Wider numeric range → slider
                return serde_json::json!({
                    "type": "slider",
                    "default_value": default_val,
                    "min": yaml_to_json(&range[0]),
                    "max": yaml_to_json(&range[1]),
                    "step": 1
                });
            }
        }
    }

    if let Some(values) = &fdef.values {
        let mapping: Map<String, JsonValue> = values
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_str().map(|s| (i.to_string(), JsonValue::from(s))))
            .collect();
        return serde_json::json!({
            "type": "discrete_map",
            "default_value": default_val,
            "values_mapping": JsonValue::Object(mapping)
        });
    }

    // Fallback: treat as an unconstrained slider
    serde_json::json!({
        "type": "slider",
        "default_value": default_val,
        "min": 0,
        "max": 255,
        "step": 1
    })
}

/// Return the "outgoing" (write-side) flat list of concrete `FieldDef`s for
/// a struct, resolving one level of `{struct: name}` references.
fn outgoing_fields<'a>(
    struct_def: &'a StructDef,
    all_structs: &'a HashMap<String, StructDef>,
) -> Vec<&'a FieldDef> {
    let field_list: &[FieldOrRef] = match struct_def {
        StructDef::Bidir { outgoing, .. } => outgoing,
        StructDef::Flat(f) => f,
    };
    resolve_fields(field_list, all_structs)
}

fn resolve_fields<'a>(
    fields: &'a [FieldOrRef],
    all_structs: &'a HashMap<String, StructDef>,
) -> Vec<&'a FieldDef> {
    let mut result = Vec::new();
    for fof in fields {
        match fof {
            FieldOrRef::Field(fd) => result.push(fd),
            FieldOrRef::Ref { struct_ref } => {
                if let Some(inner) = all_structs.get(struct_ref.as_str()) {
                    // Resolve one level; avoid infinite recursion by not calling recursively.
                    let inner_fields = match inner {
                        StructDef::Bidir { outgoing, .. } => outgoing.as_slice(),
                        StructDef::Flat(f) => f.as_slice(),
                    };
                    for fof2 in inner_fields {
                        if let FieldOrRef::Field(fd) = fof2 {
                            result.push(fd);
                        }
                    }
                }
            }
        }
    }
    result
}

// ── SetSetting helpers ────────────────────────────────────────────────────────

/// Find the name of the first API whose write struct contains a non-constant
/// field named `field_name`.
pub(crate) fn find_api_for_field(config: &DeviceConfig, field_name: &str) -> Option<String> {
    let apis = config.apis.as_ref()?;
    let structs = config.structs.as_ref()?;

    for (api_name, api_def) in apis {
        if api_def.write.is_none() {
            continue;
        }
        let Some(struct_def) = structs.get(api_name.as_str()) else {
            continue;
        };
        let fields = outgoing_fields(struct_def, structs);
        for fdef in fields {
            if fdef.constant.is_none() && fdef.name == field_name {
                return Some(api_name.clone());
            }
        }
    }
    None
}

/// Parse a JSON-encoded string value into a `FieldValue` matching the type
/// expected by `field_name` in `api_name`'s write struct.
pub(crate) fn parse_setting_value(
    config: &DeviceConfig,
    api_name: &str,
    field_name: &str,
    raw: &str,
) -> Option<FieldValue> {
    let structs = config.structs.as_ref()?;
    let struct_def = structs.get(api_name)?;
    let fields = outgoing_fields(struct_def, structs);
    let fdef = fields.into_iter().find(|f| f.name == field_name)?;

    let json_val: serde_json::Value = serde_json::from_str(raw).ok()?;

    match fdef.field_type {
        FieldType::Uint8 => Some(FieldValue::U8(json_val.as_u64()? as u8)),
        FieldType::Uint16 => Some(FieldValue::U16(json_val.as_u64()? as u16)),
        FieldType::Uint32 => Some(FieldValue::U32(json_val.as_u64()? as u32)),
        FieldType::Float32 => Some(FieldValue::F32(json_val.as_f64()? as f32)),
        FieldType::ByteArray => None, // byte-array fields are not settable via D-Bus
    }
}

/// Build the full `values` map for a `WriteApi` command.
///
/// The codec requires every non-constant field in the struct. For the field
/// being set (`changed_field`) we use `new_value`; for all other non-constant
/// fields we read the current value from `status` (flat map of
/// `field_name → {"value": ..., "type": ...}`).
pub(crate) fn build_write_values(
    config: &DeviceConfig,
    api_name: &str,
    changed_field: &str,
    new_value: FieldValue,
    status: &HashMap<String, serde_json::Value>,
) -> HashMap<String, FieldValue> {
    let mut values = HashMap::new();
    values.insert(changed_field.to_string(), new_value);

    let Some(structs) = config.structs.as_ref() else {
        return values;
    };
    let Some(struct_def) = structs.get(api_name) else {
        return values;
    };

    for fdef in outgoing_fields(struct_def, structs) {
        if fdef.constant.is_some() || fdef.name == changed_field {
            continue;
        }
        // Look up the current value from status and parse it into a FieldValue.
        let raw_val = status
            .get(&fdef.name)
            .and_then(|obj| obj.get("value"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let raw_str = raw_val.to_string();
        if let Some(fv) = parse_setting_value(config, api_name, &fdef.name, &raw_str) {
            values.insert(fdef.name.clone(), fv);
        } else {
            warn!(
                "build_write_values: cannot resolve sibling field '{}' for api '{}' — struct write may fail",
                fdef.name, api_name
            );
        }
    }
    values
}

/// Build the full `values` map for a `WriteApi` command from a partial set of
/// already-parsed field values (used during persisted-settings re-apply).
///
/// Unlike `build_write_values`, there is no "changed field" — the caller
/// supplies whatever subset it has.  Any non-constant field absent from
/// `partial` is filled with its range minimum (or 0 for fields without a
/// range), so the codec always receives a complete struct.
pub(crate) fn build_write_values_with_defaults(
    config: &DeviceConfig,
    api_name: &str,
    mut partial: HashMap<String, FieldValue>,
) -> HashMap<String, FieldValue> {
    let Some(structs) = config.structs.as_ref() else {
        return partial;
    };
    let Some(struct_def) = structs.get(api_name) else {
        return partial;
    };

    for fdef in outgoing_fields(struct_def, structs) {
        if fdef.constant.is_some() || partial.contains_key(&fdef.name) {
            continue;
        }
        let range_min_u64 = fdef
            .range
            .as_ref()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let range_min_f64 = fdef
            .range
            .as_ref()
            .and_then(|r| r.first())
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let default_fv = match fdef.field_type {
            FieldType::Uint8 => FieldValue::U8(range_min_u64 as u8),
            FieldType::Uint16 => FieldValue::U16(range_min_u64 as u16),
            FieldType::Uint32 => FieldValue::U32(range_min_u64 as u32),
            FieldType::Float32 => FieldValue::F32(range_min_f64 as f32),
            FieldType::ByteArray => continue,
        };
        warn!(
            "build_write_values_with_defaults: field '{}' for api '{}' not in persisted map, using default {:?}",
            fdef.name, api_name, default_fv
        );
        partial.insert(fdef.name.clone(), default_fv);
    }
    partial
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_version_is_semver_like() {
        let v = env!("LAM_VERSION");
        assert!(!v.is_empty());
        assert!(
            v.split('.').count() >= 2,
            "'{v}' has fewer than 2 version components"
        );
    }

    #[test]
    fn settings_changed_event_carries_json() {
        use tokio::sync::broadcast;
        let (tx, mut rx) = broadcast::channel::<SignalEvent>(4);
        let payload = r#"{"general":{},"device":{"volume":50},"settings_config":{}}"#;
        tx.send(SignalEvent::SettingsChanged {
            json: payload.to_string(),
        })
        .unwrap();
        match rx.try_recv().unwrap() {
            SignalEvent::SettingsChanged { json } => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap();
                assert_eq!(v["device"]["volume"], 50);
            }
            _ => panic!("wrong variant"),
        }
    }
    use device_config::{ApiDef, ApiOp, FieldType, Transport};
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    fn dummy_entry_with_status(
        status: HashMap<String, JsonValue>,
        cmd_tx: mpsc::Sender<DeviceCommand>,
    ) -> crate::state::DeviceEntry {
        let config = DeviceConfig::default();
        crate::state::DeviceEntry {
            config: Arc::new(config),
            vid: 0,
            pid: 0,
            status,
            cmd_tx,
        }
    }

    #[test]
    fn build_status_json_empty() {
        let state = AppState {
            configs: vec![],
            devices: HashMap::new(),
            config_dirs: vec![PathBuf::from("/tmp")],
            general_settings: crate::general_settings::GeneralSettings::default(),
            general_settings_path: PathBuf::from("/tmp/gs.yaml"),
        };
        assert_eq!(build_status_json(&state), "{}");
    }

    #[test]
    fn build_status_json_with_field() {
        let (tx, _rx) = mpsc::channel(1);
        let mut status = HashMap::new();
        status.insert(
            "battery".to_string(),
            serde_json::json!({"value": 80, "type": "uint8"}),
        );
        let mut devices = HashMap::new();
        devices.insert(
            PathBuf::from("/dev/hidraw0"),
            dummy_entry_with_status(status, tx),
        );
        let state = AppState {
            configs: vec![],
            devices,
            config_dirs: vec![PathBuf::from("/tmp")],
            general_settings: crate::general_settings::GeneralSettings::default(),
            general_settings_path: PathBuf::from("/tmp/gs.yaml"),
        };
        let json: JsonValue = serde_json::from_str(&build_status_json(&state)).unwrap();
        // Fields are nested under the "headset" category for GUI compatibility.
        assert_eq!(json["headset"]["battery"]["value"], 80);
        assert_eq!(json["headset"]["battery"]["type"], "uint8");
    }

    #[test]
    fn build_status_json_uses_representation() {
        let (tx, _rx) = mpsc::channel(1);
        let mut status = HashMap::new();
        status.insert(
            "battery".to_string(),
            serde_json::json!({"value": 80, "type": "uint8"}),
        );
        status.insert(
            "wireless_mode".to_string(),
            serde_json::json!({"value": "speed", "type": "label"}),
        );
        let mut rep = HashMap::new();
        rep.insert("headset".to_string(), vec!["battery".to_string()]);
        rep.insert("wireless".to_string(), vec!["wireless_mode".to_string()]);
        let config = DeviceConfig {
            representation: Some(rep),
            ..DeviceConfig::default()
        };
        let mut devices = HashMap::new();
        devices.insert(
            std::path::PathBuf::from("/dev/hidraw0"),
            crate::state::DeviceEntry {
                config: Arc::new(config),
                vid: 0,
                pid: 0,
                status,
                cmd_tx: tx,
            },
        );
        let state = AppState {
            configs: vec![],
            devices,
            config_dirs: vec![std::path::PathBuf::from("/tmp")],
            general_settings: crate::general_settings::GeneralSettings::default(),
            general_settings_path: std::path::PathBuf::from("/tmp/gs.yaml"),
        };
        let json: JsonValue = serde_json::from_str(&build_status_json(&state)).unwrap();
        assert_eq!(json["headset"]["battery"]["value"], 80);
        assert_eq!(json["wireless"]["wireless_mode"]["value"], "speed");
        // mic category was not in representation, so must be absent
        assert!(json.get("mic").is_none());
    }

    #[test]
    fn build_settings_json_general_section_populated() {
        use crate::general_settings::GeneralSettings;
        let state = AppState {
            configs: vec![],
            devices: HashMap::new(),
            config_dirs: vec![PathBuf::from("/tmp")],
            general_settings: GeneralSettings {
                redirect_audio_on_connect: true,
                redirect_audio_on_disconnect: false,
                redirect_audio_on_disconnect_device: Some("alsa_output.test".to_owned()),
            },
            general_settings_path: PathBuf::from("/tmp/gs.yaml"),
        };
        let json: JsonValue = serde_json::from_str(&build_settings_json(&state)).unwrap();
        assert_eq!(json["general"]["redirect_audio_on_connect"], true);
        assert_eq!(json["general"]["redirect_audio_on_disconnect"], false);
        assert_eq!(
            json["general"]["redirect_audio_on_disconnect_device"],
            "alsa_output.test"
        );
        // settings_config must include the 3 general fields
        assert_eq!(
            json["settings_config"]["redirect_audio_on_connect"]["type"],
            "toggle"
        );
        assert_eq!(
            json["settings_config"]["redirect_audio_on_disconnect_device"]["type"],
            "select"
        );
    }

    #[test]
    fn find_api_for_field_found() {
        use std::collections::HashMap as Map;

        let mut structs = Map::new();
        structs.insert(
            "set_vol".to_string(),
            StructDef::Flat(vec![FieldOrRef::Field(FieldDef {
                name: "volume".to_string(),
                field_type: FieldType::Uint8,
                constant: None,
                range: None,
                values: None,
                values_mapping: None,
                repeat: None,
                size: None,
            })]),
        );
        let mut apis = Map::new();
        apis.insert(
            "set_vol".to_string(),
            ApiDef {
                read: None,
                write: Some(ApiOp {
                    transport: Transport::HidIo,
                    chunk_size: 64,
                    payload_transform: None,
                }),
            },
        );
        let config = DeviceConfig {
            structs: Some(structs),
            apis: Some(apis),
            ..DeviceConfig::default()
        };

        assert_eq!(
            find_api_for_field(&config, "volume"),
            Some("set_vol".to_string())
        );
        assert_eq!(find_api_for_field(&config, "nonexistent"), None);
    }

    #[test]
    fn build_write_values_fills_sibling_fields_from_status() {
        use std::collections::HashMap as Map;

        let make_field = |name: &str| FieldDef {
            name: name.to_string(),
            field_type: FieldType::Uint8,
            constant: None,
            range: None,
            values: None,
            values_mapping: None,
            repeat: None,
            size: None,
        };

        let mut structs = Map::new();
        structs.insert(
            "stream_mix".to_string(),
            StructDef::Flat(vec![
                FieldOrRef::Field(make_field("stream_main")),
                FieldOrRef::Field(make_field("stream_aux")),
                FieldOrRef::Field(make_field("stream_mic")),
            ]),
        );
        let config = DeviceConfig {
            structs: Some(structs),
            ..DeviceConfig::default()
        };

        let mut status = Map::new();
        status.insert(
            "stream_aux".to_string(),
            serde_json::json!({"value": 50, "type": "uint8"}),
        );
        status.insert(
            "stream_mic".to_string(),
            serde_json::json!({"value": 80, "type": "uint8"}),
        );

        let values = build_write_values(
            &config,
            "stream_mix",
            "stream_main",
            FieldValue::U8(70),
            &status,
        );

        assert_eq!(values.get("stream_main"), Some(&FieldValue::U8(70)));
        assert_eq!(values.get("stream_aux"), Some(&FieldValue::U8(50)));
        assert_eq!(values.get("stream_mic"), Some(&FieldValue::U8(80)));
    }
}
