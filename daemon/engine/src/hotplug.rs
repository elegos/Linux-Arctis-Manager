// USB hot-plug detection via udev.
//
// `watch` streams HotplugEvent over a channel as devices are added/removed.
// `scan_existing` enumerates devices already connected at startup.
// Both functions filter to VID 0x1038 (SteelSeries) and an optional PID list.

use futures::StreamExt;
use std::path::PathBuf;
use tokio_udev::{AsyncMonitorSocket, Device, EventType, MonitorBuilder};
use tracing::{debug, info, warn};

const STEELSERIES_VID: u16 = 0x1038;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub hidraw_path: PathBuf,
    pub vid: u16,
    pub pid: u16,
    /// USB interface number from udev's `ID_USB_INTERFACE_NUM`, if available.
    pub interface_num: Option<u8>,
}

#[derive(Debug)]
pub enum HotplugEvent {
    Added(DeviceInfo),
    Removed(DeviceInfo),
}

/// Watch for hidraw add/remove udev events and forward matching ones to `tx`.
/// Matching: VID must be 0x1038; if `pid_allowlist` is non-empty, PID must
/// be in the list. Returns when the udev monitor closes or `tx` is dropped.
pub async fn watch(
    pid_allowlist: Vec<u16>,
    tx: tokio::sync::mpsc::Sender<HotplugEvent>,
) -> std::io::Result<()> {
    let mut stream =
        AsyncMonitorSocket::new(MonitorBuilder::new()?.match_subsystem("hidraw")?.listen()?)?;

    while let Some(result) = stream.next().await {
        // Extract owned data from the event before any await point.
        // Device is not Send, so it must not be held across .await.
        let hotplug_event = match result {
            Err(e) => {
                warn!("udev monitor error: {e}");
                continue;
            }
            Ok(event) => {
                let dev = event.device();
                let Some(node) = dev.devnode() else {
                    continue;
                };
                let Some((vid, pid)) = vid_pid(&dev) else {
                    debug!("no VID/PID for {}", node.display());
                    continue;
                };
                if !passes_filter(vid, pid, &pid_allowlist) {
                    continue;
                }
                let info = DeviceInfo {
                    hidraw_path: node.to_owned(),
                    vid,
                    pid,
                    interface_num: usb_interface_num(&dev),
                };
                match event.event_type() {
                    EventType::Add => {
                        info!("hotplug: added {:?}", info);
                        HotplugEvent::Added(info)
                    }
                    EventType::Remove => {
                        info!("hotplug: removed {:?}", info);
                        HotplugEvent::Removed(info)
                    }
                    _ => continue,
                }
                // event and dev drop here; only owned HotplugEvent crosses the await
            }
        };

        if tx.send(hotplug_event).await.is_err() {
            break; // engine dropped the receiver
        }
    }
    Ok(())
}

/// Enumerate hidraw devices already connected when the engine starts.
/// Synchronous; intended to be called once before entering the async event loop.
pub fn scan_existing(pid_allowlist: &[u16]) -> std::io::Result<Vec<DeviceInfo>> {
    let mut enumerator = tokio_udev::Enumerator::new()?;
    enumerator.match_subsystem("hidraw")?;

    Ok(enumerator
        .scan_devices()?
        .filter_map(|dev| {
            let path = dev.devnode()?.to_owned();
            let (vid, pid) = vid_pid(&dev)?;
            if passes_filter(vid, pid, pid_allowlist) {
                Some(DeviceInfo {
                    hidraw_path: path,
                    vid,
                    pid,
                    interface_num: usb_interface_num(&dev),
                })
            } else {
                None
            }
        })
        .collect())
}

fn passes_filter(vid: u16, pid: u16, pid_allowlist: &[u16]) -> bool {
    vid == STEELSERIES_VID && (pid_allowlist.is_empty() || pid_allowlist.contains(&pid))
}

fn vid_pid(dev: &Device) -> Option<(u16, u16)> {
    let vid_str = dev.property_value("ID_VENDOR_ID")?.to_str()?;
    let pid_str = dev.property_value("ID_MODEL_ID")?.to_str()?;
    let vid = u16::from_str_radix(vid_str, 16).ok()?;
    let pid = u16::from_str_radix(pid_str, 16).ok()?;
    Some((vid, pid))
}

fn usb_interface_num(dev: &Device) -> Option<u8> {
    let s = dev.property_value("ID_USB_INTERFACE_NUM")?.to_str()?;
    u8::from_str_radix(s.trim(), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steelseries_vid_passes_with_empty_allowlist() {
        assert!(passes_filter(0x1038, 0x12E0, &[]));
    }

    #[test]
    fn steelseries_vid_passes_when_pid_in_allowlist() {
        assert!(passes_filter(0x1038, 0x12E0, &[0x12E0, 0x12E5]));
    }

    #[test]
    fn steelseries_pid_blocked_when_not_in_allowlist() {
        assert!(!passes_filter(0x1038, 0x9999, &[0x12E0, 0x12E5]));
    }

    #[test]
    fn non_steelseries_vid_is_always_blocked() {
        assert!(!passes_filter(0x046d, 0x1234, &[]));
        assert!(!passes_filter(0x046d, 0x12E0, &[0x12E0]));
    }

    #[test]
    fn scan_existing_runs_without_error() {
        // No hardware required; only verifies the enumeration path doesn't panic.
        assert!(scan_existing(&[]).is_ok());
    }
}
