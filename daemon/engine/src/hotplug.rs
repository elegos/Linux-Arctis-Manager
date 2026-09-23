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
    // Standard udev HID properties — set for most interfaces.
    if let (Some(v), Some(p)) = (
        dev.property_value("ID_VENDOR_ID"),
        dev.property_value("ID_MODEL_ID"),
    ) {
        if let (Ok(vid), Ok(pid)) = (
            u16::from_str_radix(v.to_str().unwrap_or(""), 16),
            u16::from_str_radix(p.to_str().unwrap_or(""), 16),
        ) {
            return Some((vid, pid));
        }
    }
    // Fallback: parse VID:PID from the parent HID device's sysname.
    // Kernel names HID devices as "BBBB:VVVV:PPPP.IIII"
    // (e.g. "0003:1038:12E0.0007").  Raw/vendor HID interfaces often lack
    // the standard udev properties but are still reachable this way.
    let hid_sysname = dev.parent()?.sysname().to_str()?.to_owned();
    let parts: Vec<&str> = hid_sysname.splitn(3, ':').collect();
    if parts.len() == 3 && parts[0].len() == 4 {
        let vid = u16::from_str_radix(parts[1], 16).ok()?;
        let pid = u16::from_str_radix(parts[2].split('.').next()?, 16).ok()?;
        return Some((vid, pid));
    }
    None
}

/// Read the USB interface number for `dev`.
///
/// Prefers the udev-computed `ID_USB_INTERFACE_NUM` property, but some
/// systems don't populate it (e.g. udev's `usb_id` builtin not having run
/// for that device), which silently degrades multi-interface headsets to
/// PID-only matching and starts one `run_device` task per interface instead
/// of one. Fall back to reading `bInterfaceNumber` straight from the parent
/// USB interface's sysfs attribute, which the kernel always exposes
/// regardless of udev rules.
fn usb_interface_num(dev: &Device) -> Option<u8> {
    if let Some(s) = dev
        .property_value("ID_USB_INTERFACE_NUM")
        .and_then(|s| s.to_str())
    {
        if let Some(n) = parse_hex_interface_num(s) {
            return Some(n);
        }
    }
    let iface_dev = dev
        .parent_with_subsystem_devtype("usb", "usb_interface")
        .ok()??;
    let s = iface_dev.attribute_value("bInterfaceNumber")?.to_str()?;
    parse_hex_interface_num(s)
}

fn parse_hex_interface_num(s: &str) -> Option<u8> {
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

    #[test]
    fn parse_hex_interface_num_reads_two_digit_hex() {
        assert_eq!(parse_hex_interface_num("07"), Some(7));
        assert_eq!(parse_hex_interface_num("0a"), Some(10));
        assert_eq!(parse_hex_interface_num("ff"), Some(255));
    }

    #[test]
    fn parse_hex_interface_num_trims_whitespace() {
        assert_eq!(parse_hex_interface_num(" 07 \n"), Some(7));
    }

    #[test]
    fn parse_hex_interface_num_rejects_non_hex() {
        assert_eq!(parse_hex_interface_num("zz"), None);
        assert_eq!(parse_hex_interface_num(""), None);
    }
}
