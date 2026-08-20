// Hyprland IPC backend.
//
// Connects to Hyprland's socket2 event stream, maps window addresses to PIDs
// (via `hyprctl`), and emits FocusEvent::Focused / Closed.

use std::collections::HashMap;

use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tracing::warn;

use super::event::FocusEvent;

pub async fn run(tx: mpsc::Sender<FocusEvent>) {
    let sig = match std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        Ok(s) => s,
        Err(_) => {
            warn!("focus/hyprland: HYPRLAND_INSTANCE_SIGNATURE unset");
            return;
        }
    };
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());
    let socket_path = format!("{runtime}/hypr/{sig}/.socket2.sock");

    loop {
        let stream = match tokio::net::UnixStream::connect(&socket_path).await {
            Ok(s) => s,
            Err(e) => {
                warn!("focus/hyprland: socket {socket_path}: {e}");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        // Pre-populate address→pid map from existing clients.
        let mut addr_map: HashMap<String, (Option<u32>, String)> = HashMap::new();
        if let Ok(out) = tokio::process::Command::new("hyprctl")
            .args(["clients", "-j"])
            .output()
            .await
        {
            if let Ok(arr) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                for c in arr.as_array().into_iter().flatten() {
                    if let Some(addr) = c["address"].as_str() {
                        let pid = c["pid"].as_u64().map(|p| p as u32);
                        let class = c["class"].as_str().unwrap_or("").to_string();
                        addr_map.insert(addr.to_string(), (pid, class));
                    }
                }
            }
        }

        let mut lines = BufReader::new(stream).lines();
        loop {
            let Ok(Some(line)) = lines.next_line().await else {
                warn!("focus/hyprland: socket disconnected");
                break;
            };
            let Some((event, data)) = line.split_once(">>") else {
                continue;
            };
            match event {
                "openwindow" => {
                    // address,workspace,class,title
                    let mut parts = data.splitn(4, ',');
                    let addr = parts.next().unwrap_or("").to_string();
                    let _ = parts.next();
                    let class = parts.next().unwrap_or("").to_string();
                    let pid = pid_for_address(&addr).await;
                    addr_map.insert(addr, (pid, class));
                }
                "activewindow" => {
                    let (pid, class) = active_window().await;
                    if tx.send(FocusEvent::Focused { pid, class }).await.is_err() {
                        return;
                    }
                }
                "closewindow" => {
                    if let Some((Some(p), _)) = addr_map.remove(data) {
                        if tx.send(FocusEvent::Closed { pid: p }).await.is_err() {
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

async fn active_window() -> (Option<u32>, Option<String>) {
    let Ok(out) = tokio::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .await
    else {
        return (None, None);
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return (None, None);
    };
    let pid = json["pid"].as_u64().map(|p| p as u32);
    let class = json["class"].as_str().map(|s| s.to_string());
    (pid, class)
}

async fn pid_for_address(addr: &str) -> Option<u32> {
    let out = tokio::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .await
        .ok()?;
    let clients = serde_json::from_slice::<serde_json::Value>(&out.stdout).ok()?;
    clients.as_array()?.iter().find_map(|c| {
        let c_addr = c["address"].as_str()?;
        if c_addr.trim_start_matches("0x") == addr.trim_start_matches("0x") {
            c["pid"].as_u64().map(|p| p as u32)
        } else {
            None
        }
    })
}
