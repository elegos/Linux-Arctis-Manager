// X11 / XWayland backend.
//
// Watches _NET_ACTIVE_WINDOW and _NET_CLIENT_LIST on the root window via
// `xprop -spy -root`.  Works for native X11 sessions and hybrid
// Wayland+XWayland sessions (KDE Plasma, XFCE, etc.).

use std::collections::HashMap;

use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::event::FocusEvent;

pub async fn run(tx: mpsc::Sender<FocusEvent>) {
    loop {
        info!("focus/x11: starting xprop");
        let mut child = match tokio::process::Command::new("xprop")
            .args(["-spy", "-root", "_NET_ACTIVE_WINDOW", "_NET_CLIENT_LIST"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                warn!("focus/x11: xprop not available: {e}");
                return;
            }
        };

        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let mut prev_clients: Vec<u64> = Vec::new();
        let mut wid_pid: HashMap<u64, u32> = HashMap::new();

        loop {
            let Ok(Some(line)) = lines.next_line().await else {
                warn!("focus/x11: xprop exited");
                break;
            };

            if line.starts_with("_NET_ACTIVE_WINDOW") {
                let wid_str = line.split("# ").nth(1).unwrap_or("?").trim();
                if let Some(wid) = parse_window_id(&line) {
                    let (pid, class) = window_info(wid).await;
                    info!("focus/x11: active window wid={wid_str} pid={pid:?} class={class:?}");
                    if let Some(p) = pid {
                        wid_pid.insert(wid, p);
                    }
                    if pid.is_none() && class.is_none() {
                        // xprop returned no properties → Wayland-native app behind a
                        // synthetic XWayland window.  Scan /proc excluding known XWayland pids.
                        let xwayland_pids: Vec<u32> = wid_pid.values().copied().collect();
                        info!(
                            "focus/x11: synthetic window (Wayland-native app), {} XWayland pids known",
                            xwayland_pids.len()
                        );
                        if tx
                            .send(FocusEvent::WaylandNativeFocused { xwayland_pids })
                            .await
                            .is_err()
                        {
                            let _ = child.kill().await;
                            return;
                        }
                    } else if tx.send(FocusEvent::Focused { pid, class }).await.is_err() {
                        let _ = child.kill().await;
                        return;
                    }
                } else {
                    info!("focus/x11: active window cleared (wid=0): {wid_str}");
                }
            } else if line.starts_with("_NET_CLIENT_LIST") {
                let current = parse_window_list(&line);
                let removed: Vec<u64> = prev_clients
                    .iter()
                    .filter(|w| !current.contains(w))
                    .copied()
                    .collect();
                for wid in removed {
                    if let Some(pid) = wid_pid.remove(&wid) {
                        if tx.send(FocusEvent::Closed { pid }).await.is_err() {
                            let _ = child.kill().await;
                            return;
                        }
                    }
                }
                prev_clients = current;
            }
        }

        let _ = child.kill().await;
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

fn parse_window_id(line: &str) -> Option<u64> {
    // "_NET_ACTIVE_WINDOW(WINDOW): window id # 0x4400003"
    let s = line.split_once("# ")?.1.trim();
    let wid = u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()?;
    if wid == 0 {
        None
    } else {
        Some(wid)
    }
}

fn parse_window_list(line: &str) -> Vec<u64> {
    // "_NET_CLIENT_LIST(WINDOW): window id # 0x..., 0x..., ..."
    let after = line.split_once("# ").map(|(_, s)| s).unwrap_or("");
    after
        .split(',')
        .filter_map(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .collect()
}

async fn window_info(wid: u64) -> (Option<u32>, Option<String>) {
    let Ok(out) = tokio::process::Command::new("xprop")
        .args([
            "-id",
            &format!("0x{wid:x}"),
            "_NET_WM_PID",
            "WM_CLASS",
            "_KDE_NET_WM_DESKTOP_FILE",
        ])
        .output()
        .await
    else {
        return (None, None);
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pid: Option<u32> = None;
    let mut wm_instance: Option<String> = None; // first WM_CLASS component
    let mut wm_class: Option<String> = None; // second WM_CLASS component (app name)
    let mut desktop_file: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("_NET_WM_PID") {
            // _NET_WM_PID(CARDINAL) = 12345
            pid = line.split('=').nth(1).and_then(|s| s.trim().parse().ok());
        } else if line.starts_with("WM_CLASS") {
            // WM_CLASS(STRING) = "steamwebhelper", "Steam"
            //                     ^instance          ^class (app name)
            let mut parts = line.split('"').skip(1);
            wm_instance = parts.next().map(|s| s.to_string());
            parts.next(); // skip inter-quote separator
            wm_class = parts.next().map(|s| s.to_string());
        } else if line.starts_with("_KDE_NET_WM_DESKTOP_FILE") {
            // _KDE_NET_WM_DESKTOP_FILE(UTF8_STRING) = "firefox"
            // Set by KWin for Wayland-native windows that have no WM_CLASS.
            desktop_file = line.split('"').nth(1).map(|s| s.to_string());
        }
    }
    // Prefer WM_CLASS class (2nd) > instance (1st) > KDE desktop file name.
    // The class component ("Steam", "firefox") matches user-visible app names;
    // the instance ("steamwebhelper") is an internal process name that often differs.
    let class = wm_class.or(wm_instance).or(desktop_file);
    (pid, class)
}
