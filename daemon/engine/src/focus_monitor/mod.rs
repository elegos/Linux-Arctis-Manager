// Focus monitor: applies hardware-backend EQ app overrides based on window focus.
//
// Channels with `backend = Hardware` or `backend = Auto` are handled here when
// executable/steam matchers match; LADSPA channels are excluded.
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

use device_config::codec::FieldValue;

use crate::audio::AudioSetup;
use crate::eq::settings::{
    load_eq_settings, AppMatcher, Channel, ChannelEqSettings, EqBackend, EqSettings,
};
use crate::eq_manager::{self as eq_manager, EqRuntime};
use crate::state::{AppState, DeviceCommand, SignalEvent};

pub use event::FocusEvent;

/// Delay before retrying a failed IPC connection attempt (sway/hyprland).
const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(5);
/// Delay before restarting a backend's event loop after the connection was lost.
const DISCONNECT_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

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
         Hyprland, Sway, KDE Plasma, or an X11/XWayland session is required \
         for hardware EQ app overrides."
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

/// Borrows both per-channel focus stacks so event handlers can pick the right
/// one for each channel without threading two separate `&mut` parameters
/// through every call.
struct ChannelStacks<'a> {
    media: &'a mut ChannelStack,
    chat: &'a mut ChannelStack,
}

impl<'a> ChannelStacks<'a> {
    fn get(&mut self, channel: Channel) -> &mut ChannelStack {
        match channel {
            Channel::Media => self.media,
            Channel::Chat => self.chat,
        }
    }
}

/// Writes a factory hardware EQ preset directly (bypasses the LADSPA/custom-slot path).
async fn write_hw_preset(ctx: &eq_manager::HwEqContext, hw_idx: u8) {
    let values = std::collections::HashMap::from([("eq_preset".to_string(), FieldValue::U8(hw_idx))]);
    let _ = ctx
        .cmd_tx
        .send(DeviceCommand::WriteApi {
            api_name: "selected_eq_preset".into(),
            values,
        })
        .await;
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
                // Walk up the process tree (up to 5 levels) so that child processes
                // (e.g. steamwebhelper) match against their parent (steam).
                if exe_chain_matches(p, base) {
                    return true;
                }
            }
            class.is_some_and(|c| c.eq_ignore_ascii_case(base))
        }
        AppMatcher::SteamGame { app_id } => {
            pid.and_then(steam_app_id_for_pid).as_deref() == Some(&app_id.to_string())
        }
    }
}

/// Walk /proc/{pid}/exe and up the parent chain (PPid from /proc/{pid}/status)
/// returning true if any ancestor's exe basename matches `target`.
fn exe_chain_matches(pid: u32, target: &str) -> bool {
    let mut cur = pid;
    for _ in 0..5 {
        if let Ok(exe) = std::fs::read_link(format!("/proc/{cur}/exe")) {
            if exe.file_name().and_then(|n| n.to_str()) == Some(target) {
                return true;
            }
        }
        // Read PPid from /proc/{cur}/status
        let status = match std::fs::read_to_string(format!("/proc/{cur}/status")) {
            Ok(s) => s,
            Err(_) => break,
        };
        let ppid = status
            .lines()
            .find(|l| l.starts_with("PPid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        if ppid <= 1 {
            break;
        }
        cur = ppid;
    }
    false
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
                let stacks = ChannelStacks { media: &mut media_stack, chat: &mut chat_stack };
                match ev {
                    FocusEvent::Focused { pid, class } => {
                        tracing::info!(
                            "focus monitor: focused pid={pid:?} class={class:?}"
                        );
                        on_focused(pid, class.as_deref(), &eq_settings,
                            stacks,
                            &settings_base_dir, &audio_shared, &eq_rt, hw_ctx.as_ref()).await;
                    }
                    FocusEvent::Closed { pid } => {
                        tracing::info!("focus monitor: closed pid={pid}");
                        on_closed(pid, &eq_settings,
                            stacks,
                            &settings_base_dir, &audio_shared, &eq_rt, hw_ctx.as_ref()).await;
                    }
                    FocusEvent::WaylandNativeFocused { xwayland_pids } => {
                        tracing::info!(
                            "focus monitor: Wayland-native window focused ({} XWayland pids excluded)",
                            xwayland_pids.len()
                        );
                        on_wayland_native_focused(&xwayland_pids, &eq_settings,
                            stacks,
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
    mut stacks: ChannelStacks<'_>,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    for channel in [Channel::Media, Channel::Chat] {
        process_channel_focus(
            pid,
            class,
            channel.settings(settings),
            channel,
            stacks.get(channel),
            base_dir,
            audio,
            eq_rt,
            hw_ctx,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_channel_focus(
    pid: Option<u32>,
    class: Option<&str>,
    ch: &ChannelEqSettings,
    channel: Channel,
    stack: &mut ChannelStack,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    if matches!(ch.backend, EqBackend::Ladspa) {
        return; // focus monitor only drives HW and Auto paths
    }
    let Some(ov) = ch
        .app_overrides
        .iter()
        .find(|o| matches_focus(pid, class, &o.matcher))
    else {
        return;
    };
    if let Some(hw_idx) = ov.hw_preset_idx {
        // Factory preset override: write selected_eq_preset directly.
        if let Some(ctx) = hw_ctx {
            write_hw_preset(ctx, hw_idx).await;
        }
        return;
    }
    if let Some(new_preset) = stack.on_focus(pid, ov.preset.clone()) {
        let mut apply = ch.clone();
        apply.preset = new_preset;
        apply.enabled = true;
        eq_manager::apply_channel_eq(&apply, channel, base_dir, audio, eq_rt, hw_ctx).await;
    }
}

async fn on_closed(
    pid: u32,
    settings: &EqSettings,
    mut stacks: ChannelStacks<'_>,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    for channel in [Channel::Media, Channel::Chat] {
        process_channel_close(
            pid,
            channel.settings(settings),
            channel,
            stacks.get(channel),
            base_dir,
            audio,
            eq_rt,
            hw_ctx,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_channel_close(
    pid: u32,
    ch: &ChannelEqSettings,
    channel: Channel,
    stack: &mut ChannelStack,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    if matches!(ch.backend, EqBackend::Ladspa) {
        return; // focus monitor only drives HW and Auto paths
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

// ── Wayland-native window fallback (x11 synthetic window + /proc scan) ────────

/// Scan /proc for a running process whose exe basename matches `name` and whose
/// PID is not in `excluded` (the known XWayland process list).
fn find_proc_by_exe(name: &str, excluded: &[u32]) -> Option<u32> {
    let dir = std::fs::read_dir("/proc").ok()?;
    for entry in dir.flatten() {
        let fname = entry.file_name();
        let Some(pid) = fname.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if excluded.contains(&pid) {
            continue;
        }
        if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
            if exe.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(pid);
            }
        }
    }
    None
}

async fn on_wayland_native_focused(
    xwayland_pids: &[u32],
    settings: &EqSettings,
    mut stacks: ChannelStacks<'_>,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    for channel in [Channel::Media, Channel::Chat] {
        process_channel_wayland_native(
            xwayland_pids,
            channel.settings(settings),
            channel,
            stacks.get(channel),
            base_dir,
            audio,
            eq_rt,
            hw_ctx,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_channel_wayland_native(
    xwayland_pids: &[u32],
    ch: &ChannelEqSettings,
    channel: Channel,
    stack: &mut ChannelStack,
    base_dir: &Path,
    audio: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    if matches!(ch.backend, EqBackend::Ladspa) {
        return;
    }
    for ov in &ch.app_overrides {
        let AppMatcher::Executable { path } = &ov.matcher else {
            continue; // SteamGame overrides are always XWayland
        };
        let base = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let Some(pid) = find_proc_by_exe(base, xwayland_pids) else {
            continue;
        };
        tracing::info!("focus/x11: Wayland-native match exe={base} pid={pid}");
        if let Some(hw_idx) = ov.hw_preset_idx {
            if let Some(ctx) = hw_ctx {
                write_hw_preset(ctx, hw_idx).await;
            }
            return;
        }
        if let Some(new_preset) = stack.on_focus(Some(pid), ov.preset.clone()) {
            let mut apply = ch.clone();
            apply.preset = new_preset;
            apply.enabled = true;
            eq_manager::apply_channel_eq(&apply, channel, base_dir, audio, eq_rt, hw_ctx).await;
        }
        return; // first match wins
    }
}
