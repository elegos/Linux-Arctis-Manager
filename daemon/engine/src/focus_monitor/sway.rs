// Sway (i3-compatible) IPC backend.
//
// Subscribes to the Sway IPC window event stream.  Sway includes the container
// PID in every window event, so no secondary lookup is needed.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::warn;

use super::event::FocusEvent;

// i3/Sway IPC window event type (high bit set | 3)
const IPC_EVENT_WINDOW: u32 = 0x80000003;

pub async fn run(tx: mpsc::Sender<FocusEvent>) {
    let sock_path = match std::env::var("SWAYSOCK") {
        Ok(s) => s,
        Err(_) => {
            warn!("focus/sway: SWAYSOCK unset");
            return;
        }
    };

    loop {
        let mut stream = match tokio::net::UnixStream::connect(&sock_path).await {
            Ok(s) => s,
            Err(e) => {
                warn!("focus/sway: socket {sock_path}: {e}");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        let sub = encode(2, br#"["window"]"#);
        if stream.write_all(&sub).await.is_err() {
            continue;
        }
        let _ = decode(&mut stream).await; // discard subscribe reply

        loop {
            let Some((msg_type, payload)) = decode(&mut stream).await else {
                warn!("focus/sway: connection lost");
                break;
            };
            if msg_type != IPC_EVENT_WINDOW {
                continue;
            }
            let Ok(json) = serde_json::from_slice::<serde_json::Value>(&payload) else {
                continue;
            };
            let change = json["change"].as_str().unwrap_or("");
            let pid = json["container"]["pid"].as_u64().map(|p| p as u32);
            let app_id = json["container"]["app_id"]
                .as_str()
                .or_else(|| json["container"]["window_properties"]["class"].as_str())
                .map(|s| s.to_string());

            let ev = match change {
                "focus" => FocusEvent::Focused { pid, class: app_id },
                "close" => match pid {
                    Some(p) => FocusEvent::Closed { pid: p },
                    None => continue,
                },
                _ => continue,
            };
            if tx.send(ev).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

fn encode(msg_type: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = b"i3-ipc".to_vec();
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&msg_type.to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

async fn decode(stream: &mut tokio::net::UnixStream) -> Option<(u32, Vec<u8>)> {
    let mut hdr = [0u8; 14]; // "i3-ipc"(6) + len(4) + type(4)
    stream.read_exact(&mut hdr).await.ok()?;
    if &hdr[..6] != b"i3-ipc" {
        return None;
    }
    let len = u32::from_le_bytes(hdr[6..10].try_into().unwrap()) as usize;
    let msg_type = u32::from_le_bytes(hdr[10..14].try_into().unwrap());
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.ok()?;
    Some((msg_type, payload))
}
