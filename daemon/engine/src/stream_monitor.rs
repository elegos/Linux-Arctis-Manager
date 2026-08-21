// Watches PipeWire/PulseAudio client events and activates per-app EQ overrides.
//
// Subscribes to `pactl subscribe` for reactivity; on each client add/remove
// event re-snapshots the live client list and compares it against the
// channel-level `app_overrides` in `EqSettings`.  When a match is found the
// override preset is applied via `eq_manager`; when the matching client
// disappears the channel default is restored.
//
// Handles the LADSPA (software) path and factory-preset hardware overrides.
// Software-preset overrides on hardware-backend channels are driven by focus_monitor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

use device_config::codec::FieldValue;

use crate::audio::AudioSetup;
use crate::eq::settings::{load_eq_settings, AppMatcher, ChannelEqSettings, EqBackend, EqSettings};
use crate::eq_manager::{self as eq_manager, EqRuntime};
use crate::state::{AppState, DeviceCommand, SignalEvent};

// ── Client snapshot ───────────────────────────────────────────────────────────

struct PwClient {
    name: String,
    binary: String,
    pid: u32,
}

async fn list_pw_clients() -> Vec<PwClient> {
    let out = tokio::process::Command::new("pactl")
        .args(["-f", "json", "list", "clients"])
        .output()
        .await;
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        return vec![];
    }
    let Ok(arr) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return vec![];
    };
    let Some(arr) = arr.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|c| {
            let props = &c["properties"];
            let name = props["application.name"].as_str()?.to_owned();
            if name.starts_with("pipewire") || name.starts_with("PulseAudio") {
                return None;
            }
            let binary = props["application.process.binary"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let pid = props["application.process.id"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            Some(PwClient { name, binary, pid })
        })
        .collect()
}

// ── Matcher ───────────────────────────────────────────────────────────────────

fn matches(client: &PwClient, matcher: &AppMatcher) -> bool {
    match matcher {
        AppMatcher::Stream { name } => client.name == *name,
        AppMatcher::Executable { path } => {
            let base = Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path.as_str());
            client.binary == base
        }
        AppMatcher::SteamGame { app_id } => steam_app_id_for_pid(client.pid) == Some(*app_id),
    }
}

/// Read `SteamAppId` from `/proc/<pid>/environ` to identify a Steam game client.
fn steam_app_id_for_pid(pid: u32) -> Option<u32> {
    let env = std::fs::read_to_string(format!("/proc/{pid}/environ")).ok()?;
    env.split('\0')
        .find_map(|var| var.strip_prefix("SteamAppId=")?.parse().ok())
}

// ── Override application ──────────────────────────────────────────────────────

/// Check running clients against app_overrides and apply/restore as needed.
///
/// `active[channel]` = `Some(preset)` when an override is currently in effect.
async fn check_and_apply(
    base_dir: &Path,
    settings: &EqSettings,
    audio_shared: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    app_state: &Arc<Mutex<AppState>>,
    active: &mut HashMap<String, Option<String>>,
) {
    let clients = list_pw_clients().await;
    let hw_ctx = {
        let st = app_state.lock().await;
        eq_manager::build_hw_eq_context(&st)
    };

    for (channel, ch_settings) in [("media", &settings.media), ("chat", &settings.chat)] {
        // Hardware-backend channels: skip unless there are factory-preset overrides.
        // Software-preset overrides on hardware channels are handled by focus_monitor.
        let is_hw_backend = matches!(ch_settings.backend, EqBackend::Hardware);
        if is_hw_backend
            && ch_settings
                .app_overrides
                .iter()
                .all(|o| o.hw_preset_idx.is_none())
        {
            continue;
        }
        if ch_settings.app_overrides.is_empty() {
            continue;
        }

        // First override whose matcher matches any live client wins.
        let matched_ov = ch_settings
            .app_overrides
            .iter()
            .find(|ov| clients.iter().any(|c| matches(c, &ov.matcher)));

        // Derive a tracking key: "hw:{idx}" for factory presets, preset name for software.
        let matched_key: Option<String> = matched_ov.map(|ov| {
            if let Some(idx) = ov.hw_preset_idx {
                format!("hw:{idx}")
            } else {
                ov.preset.clone()
            }
        });

        let currently = active.get(channel).and_then(|v| v.as_deref());

        match (matched_key.as_deref(), currently) {
            // Override unchanged — nothing to do.
            (Some(k), Some(c)) if k == c => {}
            (None, None) => {}

            // New or changed override.
            (Some(key), _) => {
                info!("stream monitor: {channel} → override '{key}'");
                let ov = matched_ov.unwrap();
                if let Some(idx) = ov.hw_preset_idx {
                    // Factory preset override: write selected_eq_preset directly.
                    if let Some(ctx) = &hw_ctx {
                        let values = std::collections::HashMap::from([(
                            "eq_preset".to_string(),
                            FieldValue::U8(idx),
                        )]);
                        let _ = ctx
                            .cmd_tx
                            .send(DeviceCommand::WriteApi {
                                api_name: "selected_eq_preset".into(),
                                values,
                            })
                            .await;
                    }
                } else if !is_hw_backend {
                    // Software preset override: apply via LADSPA path.
                    let mut ovr = ch_settings.clone();
                    ovr.preset = ov.preset.clone();
                    ovr.enabled = true;
                    eq_manager::apply_channel_eq(
                        &ovr,
                        channel,
                        base_dir,
                        audio_shared,
                        eq_rt,
                        hw_ctx.as_ref(),
                    )
                    .await;
                }
                active.insert(channel.to_string(), Some(key.to_owned()));
            }

            // Override lifted — restore channel default.
            (None, Some(prev)) => {
                info!("stream monitor: {channel} → restoring default");
                // For both factory and software overrides, restore via the normal path.
                if !prev.starts_with("hw:") && is_hw_backend {
                    // Software override on hw-backend — focus_monitor owns this, skip.
                } else {
                    restore_channel(
                        ch_settings,
                        channel,
                        base_dir,
                        audio_shared,
                        eq_rt,
                        hw_ctx.as_ref(),
                    )
                    .await;
                }
                active.insert(channel.to_string(), None);
            }
        }
    }
}

async fn restore_channel(
    ch: &ChannelEqSettings,
    channel: &str,
    base_dir: &Path,
    audio_shared: &Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: &Arc<Mutex<EqRuntime>>,
    hw_ctx: Option<&eq_manager::HwEqContext>,
) {
    if ch.enabled {
        eq_manager::apply_channel_eq(ch, channel, base_dir, audio_shared, eq_rt, hw_ctx).await;
    } else {
        eq_manager::disable_channel_eq(channel, audio_shared, eq_rt, hw_ctx).await;
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run(
    settings_base_dir: PathBuf,
    audio_shared: Arc<Mutex<Option<AudioSetup>>>,
    eq_rt: Arc<Mutex<EqRuntime>>,
    app_state: Arc<Mutex<AppState>>,
    mut signal_rx: broadcast::Receiver<SignalEvent>,
) {
    let mut eq_settings = load_eq_settings(&settings_base_dir);
    let mut active: HashMap<String, Option<String>> = HashMap::new();

    let child = tokio::process::Command::new("pactl")
        .arg("subscribe")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            warn!("stream monitor: cannot start pactl subscribe: {e}");
            return;
        }
    };

    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();

    // Initial check in case streams are already running.
    check_and_apply(
        &settings_base_dir,
        &eq_settings,
        &audio_shared,
        &eq_rt,
        &app_state,
        &mut active,
    )
    .await;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(l)) if l.contains("client") => {
                        check_and_apply(
                            &settings_base_dir, &eq_settings,
                            &audio_shared, &eq_rt, &app_state, &mut active,
                        ).await;
                    }
                    Ok(None) | Err(_) => {
                        warn!("stream monitor: pactl subscribe exited");
                        break;
                    }
                    Ok(Some(_)) => {} // sink/source/etc events — ignore
                }
            }
            event = signal_rx.recv() => {
                match event {
                    Ok(SignalEvent::EQChanged { json }) => {
                        if let Ok(s) = serde_json::from_str::<EqSettings>(&json) {
                            eq_settings = s;
                        }
                        check_and_apply(
                            &settings_base_dir, &eq_settings,
                            &audio_shared, &eq_rt, &app_state, &mut active,
                        ).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }

    let _ = child.kill().await;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::settings::AppMatcher;

    fn client(name: &str, binary: &str, pid: u32) -> PwClient {
        PwClient {
            name: name.to_owned(),
            binary: binary.to_owned(),
            pid,
        }
    }

    #[test]
    fn stream_matcher_matches_by_name() {
        let c = client("Spotify", "spotify", 1234);
        assert!(matches(
            &c,
            &AppMatcher::Stream {
                name: "Spotify".into()
            }
        ));
        assert!(!matches(
            &c,
            &AppMatcher::Stream {
                name: "Firefox".into()
            }
        ));
    }

    #[test]
    fn executable_matcher_uses_basename() {
        let c = client("Firefox", "firefox", 1234);
        assert!(matches(
            &c,
            &AppMatcher::Executable {
                path: "/usr/bin/firefox".into()
            }
        ));
        assert!(matches(
            &c,
            &AppMatcher::Executable {
                path: "firefox".into()
            }
        ));
        assert!(!matches(
            &c,
            &AppMatcher::Executable {
                path: "/usr/bin/spotify".into()
            }
        ));
    }

    #[test]
    fn steam_matcher_no_pid_returns_false() {
        // PID 0 will fail /proc read; should return false gracefully.
        let c = client("GameApp", "game", 0);
        assert!(!matches(&c, &AppMatcher::SteamGame { app_id: 730 }));
    }
}
