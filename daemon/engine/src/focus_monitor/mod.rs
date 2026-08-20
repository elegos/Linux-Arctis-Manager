// Focus monitor: applies hardware-backend EQ app overrides based on window focus.
//
// Only channels with `backend = Hardware` are handled here; LADSPA and Auto
// channels remain under stream_monitor.
//
// Backend implementations all satisfy the same contract:
//   `pub async fn run(tx: mpsc::Sender<FocusEvent>)`
// The orchestrator (this module) detects the active backend at runtime, spawns
// it, and drives the per-channel focus stack.

mod event;
mod hyprland;
mod sway;
mod x11;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::info;

use crate::audio::AudioSetup;
use crate::eq::settings::{load_eq_settings, AppMatcher, ChannelEqSettings, EqBackend, EqSettings};
use crate::eq_manager::{self as eq_manager, EqRuntime};
use crate::state::{AppState, SignalEvent};

pub use event::FocusEvent;

// ── Backend ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum FocusBackend {
    Hyprland,
    Sway,
    X11,
    Unsupported(String),
}

/// Detect the best available focus-tracking backend for this session.
pub fn detect() -> FocusBackend {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        return FocusBackend::Hyprland;
    }
    if std::env::var("SWAYSOCK").is_ok() {
        return FocusBackend::Sway;
    }
    if std::env::var("DISPLAY").is_ok() {
        return FocusBackend::X11;
    }
    let de = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if de.contains("gnome") {
        return FocusBackend::Unsupported(
            "GNOME Wayland does not expose active window information. \
             Hardware EQ app overrides are unavailable on this session."
                .to_string(),
        );
    }
    FocusBackend::Unsupported(format!(
        "No supported focus-tracking method found (desktop: \"{de}\"). \
         Hyprland, Sway, or an X11/XWayland session is required for hardware EQ app overrides."
    ))
}

pub fn backend_id(b: &FocusBackend) -> &'static str {
    match b {
        FocusBackend::Hyprland => "hyprland",
        FocusBackend::Sway => "sway",
        FocusBackend::X11 => "x11",
        FocusBackend::Unsupported(_) => "unsupported",
    }
}

pub fn unsupported_reason(b: &FocusBackend) -> Option<&str> {
    match b {
        FocusBackend::Unsupported(r) => Some(r.as_str()),
        _ => None,
    }
}

// ── Focus stack ───────────────────────────────────────────────────────────────

#[derive(Default)]
struct ChannelStack(Vec<StackEntry>);

struct StackEntry {
    pid: Option<u32>,
    preset: String,
}

enum RestoreAction {
    Apply(String),
    Default,
}

impl ChannelStack {
    fn cleanup_dead(v: &mut Vec<StackEntry>) {
        v.retain(|e| {
            e.pid
                .is_none_or(|p| Path::new(&format!("/proc/{p}")).exists())
        });
    }

    fn top_preset(&self) -> Option<&str> {
        self.0.last().map(|e| e.preset.as_str())
    }

    /// Move pid/preset to stack top.  Returns the new top preset if it changed.
    fn on_focus(&mut self, pid: Option<u32>, preset: String) -> Option<String> {
        let old = self.top_preset().map(|s| s.to_owned());
        if let Some(p) = pid {
            self.0.retain(|e| e.pid != Some(p));
        }
        self.0.push(StackEntry { pid, preset });
        let new = self.top_preset().map(|s| s.to_owned());
        if new != old {
            new
        } else {
            None
        }
    }

    /// Remove a closed pid and clean dead entries.  Returns RestoreAction if top changed.
    fn on_close(&mut self, pid: u32) -> Option<RestoreAction> {
        let old = self.top_preset().map(|s| s.to_owned());
        self.0.retain(|e| e.pid != Some(pid));
        Self::cleanup_dead(&mut self.0);
        let new = self.top_preset().map(|s| s.to_owned());
        if new != old {
            Some(match self.0.last() {
                Some(e) => RestoreAction::Apply(e.preset.clone()),
                None => RestoreAction::Default,
            })
        } else {
            None
        }
    }
}

// ── App matching ──────────────────────────────────────────────────────────────

fn matches_focus(pid: Option<u32>, class: Option<&str>, matcher: &AppMatcher) -> bool {
    match matcher {
        AppMatcher::Stream { .. } => false, // PipeWire streams not relevant for HW path
        AppMatcher::Executable { path } => {
            let base = Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if let Some(p) = pid {
                if let Ok(exe) = std::fs::read_link(format!("/proc/{p}/exe")) {
                    if exe.file_name().and_then(|n| n.to_str()) == Some(base) {
                        return true;
                    }
                }
            }
            class.is_some_and(|c| c.eq_ignore_ascii_case(base))
        }
        AppMatcher::SteamGame { app_id } => {
            pid.and_then(steam_app_id_for_pid).as_deref() == Some(&app_id.to_string())
        }
    }
}

fn steam_app_id_for_pid(pid: u32) -> Option<String> {
    let data = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    data.split(|&b| b == 0)
        .find(|v| v.starts_with(b"SteamAppId="))
        .and_then(|v| std::str::from_utf8(&v[11..]).ok())
        .map(|s| s.to_string())
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run(
    settings_base_dir: PathBuf,
    audio_shared: Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: Arc<Mutex<EqRuntime>>,
    app_state: Arc<Mutex<AppState>>,
    mut signal_rx: broadcast::Receiver<SignalEvent>,
) {
    let backend = detect();
    info!("focus monitor: backend = {}", backend_id(&backend));

    if let FocusBackend::Unsupported(ref reason) = backend {
        info!("focus monitor: disabled — {reason}");
        return;
    }

    let (ev_tx, mut ev_rx) = mpsc::channel::<FocusEvent>(32);

    match backend {
        FocusBackend::Hyprland => tokio::spawn(hyprland::run(ev_tx)),
        FocusBackend::Sway => tokio::spawn(sway::run(ev_tx)),
        FocusBackend::X11 => tokio::spawn(x11::run(ev_tx)),
        FocusBackend::Unsupported(_) => unreachable!(),
    };

    let mut eq_settings = load_eq_settings(&settings_base_dir);
    let mut media_stack = ChannelStack::default();
    let mut chat_stack = ChannelStack::default();

    loop {
        tokio::select! {
            ev = ev_rx.recv() => {
                let Some(ev) = ev else { break };
                let hw_ctx = { let st = app_state.lock().await; eq_manager::build_hw_eq_context(&st) };
                match ev {
                    FocusEvent::Focused { pid, class } => {
                        on_focused(pid, class.as_deref(), &eq_settings,
                            &mut media_stack, &mut chat_stack,
                            &settings_base_dir, &audio_shared, &eq_rt, hw_ctx.as_ref()).await;
                    }
                    FocusEvent::Closed { pid } => {
                        on_closed(pid, &eq_settings,
                            &mut media_stack, &mut chat_stack,
                            &settings_base_dir, &audio_shared, &eq_rt, hw_ctx.as_ref()).await;
                    }
                }
            }
            event = signal_rx.recv() => {
                match event {
                    Ok(SignalEvent::EQChanged { json }) => {
                        if let Ok(s) = serde_json::from_str::<EqSettings>(&json) {
                            eq_settings = s;
                        }
                        media_stack = ChannelStack::default();
                        chat_stack = ChannelStack::default();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }
}

// ── Event handlers ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn on_focused(
    pid: Option<u32>,
    class: Option<&str>,
    settings: &EqSettings,
    media_stack: &mut ChannelStack,
    chat_stack: &mut ChannelStack,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    process_channel_focus(
        pid,
        class,
        &settings.media,
        "media",
        media_stack,
        base_dir,
        audio,
        eq_rt,
        hw_ctx,
    )
    .await;
    process_channel_focus(
        pid,
        class,
        &settings.chat,
        "chat",
        chat_stack,
        base_dir,
        audio,
        eq_rt,
        hw_ctx,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn process_channel_focus(
    pid: Option<u32>,
    class: Option<&str>,
    ch: &ChannelEqSettings,
    channel: &str,
    stack: &mut ChannelStack,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    if !matches!(ch.backend, EqBackend::Hardware) {
        return;
    }
    let Some(ov) = ch
        .app_overrides
        .iter()
        .find(|o| matches_focus(pid, class, &o.matcher))
    else {
        return;
    };
    if let Some(new_preset) = stack.on_focus(pid, ov.preset.clone()) {
        let mut apply = ch.clone();
        apply.preset = new_preset;
        apply.enabled = true;
        eq_manager::apply_channel_eq(&apply, channel, base_dir, audio, eq_rt, hw_ctx).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn on_closed(
    pid: u32,
    settings: &EqSettings,
    media_stack: &mut ChannelStack,
    chat_stack: &mut ChannelStack,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    process_channel_close(
        pid,
        &settings.media,
        "media",
        media_stack,
        base_dir,
        audio,
        eq_rt,
        hw_ctx,
    )
    .await;
    process_channel_close(
        pid,
        &settings.chat,
        "chat",
        chat_stack,
        base_dir,
        audio,
        eq_rt,
        hw_ctx,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn process_channel_close(
    pid: u32,
    ch: &ChannelEqSettings,
    channel: &str,
    stack: &mut ChannelStack,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    if !matches!(ch.backend, EqBackend::Hardware) {
        return;
    }
    let Some(action) = stack.on_close(pid) else {
        return;
    };
    match action {
        RestoreAction::Apply(preset) => {
            let mut apply = ch.clone();
            apply.preset = preset;
            apply.enabled = true;
            eq_manager::apply_channel_eq(&apply, channel, base_dir, audio, eq_rt, hw_ctx).await;
        }
        RestoreAction::Default => {
            if ch.enabled {
                eq_manager::apply_channel_eq(ch, channel, base_dir, audio, eq_rt, hw_ctx).await;
            } else {
                eq_manager::disable_channel_eq(channel, audio, eq_rt, hw_ctx).await;
            }
        }
    }
}
