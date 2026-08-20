// X11 / XWayland backend.
//
// Watches _NET_ACTIVE_WINDOW and _NET_CLIENT_LIST on the root window via
// `xprop -spy -root`.  Works for native X11 sessions and hybrid
// Wayland+XWayland sessions (KDE Plasma, XFCE, etc.).

use std::collections::HashMap;

use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tracing::warn;

use super::event::FocusEvent;

pub async fn run(tx: mpsc::Sender<FocusEvent>) {
    loop {
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
                if let Some(wid) = parse_window_id(&line) {
                    let (pid, class) = window_info(wid).await;
                    if let Some(p) = pid {
                        wid_pid.insert(wid, p);
                    }
                    if tx.send(FocusEvent::Focused { pid, class }).await.is_err() {
                        let _ = child.kill().await;
                        return;
                    }
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
        .args(["-id", &format!("0x{wid:x}"), "_NET_WM_PID", "WM_CLASS"])
        .output()
        .await
    else {
        return (None, None);
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pid: Option<u32> = None;
    let mut class: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("_NET_WM_PID") {
            // _NET_WM_PID(CARDINAL) = 12345
            pid = line.split('=').nth(1).and_then(|s| s.trim().parse().ok());
        } else if line.starts_with("WM_CLASS") {
            // WM_CLASS(STRING) = "instance", "Class"  — use instance name (first)
            class = line.split('"').nth(1).map(|s| s.to_string());
        }
    }
    (pid, class)
}
