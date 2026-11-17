// Read-only enumeration of the SteelSeries hidraw nodes, for the GUI's
// diagnostics page. It applies the same checks the helper applies before
// handing out a descriptor (it calls the very same functions), so the report
// says exactly why a node would be refused, but it never opens a device node.
// Only SteelSeries nodes are reported: nothing is said about other hardware.

use crate::server::{has_audio_interface, hid_device_path, is_audio_accessory, usb_device_of};
use std::path::Path;

const STEELSERIES_VID: u16 = 0x1038;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The helper would open this node (USB audio product, or a listed accessory).
    Accessible,
    /// SteelSeries device that is not an audio product (keyboard, mouse, ...).
    NotAudioProduct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidrawNode {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub interface_number: Option<u8>,
    pub has_audio_interface: bool,
    pub is_audio_accessory: bool,
    pub verdict: Verdict,
    /// HID report descriptor, lowercase hex, when sysfs exposes it.
    pub report_descriptor_hex: Option<String>,
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_hex_u16(path: &Path) -> Option<u16> {
    u16::from_str_radix(read_trimmed(path)?.trim_start_matches("0x"), 16).ok()
}

fn describe(sysfs_base: &Path, name: &str) -> Option<HidrawNode> {
    let hid_dev = hid_device_path(sysfs_base, name).ok()?;
    let usb_dev = usb_device_of(&hid_dev).ok()?;
    let vid = read_hex_u16(&usb_dev.join("idVendor"))?;
    if vid != STEELSERIES_VID {
        return None;
    }
    let pid = read_hex_u16(&usb_dev.join("idProduct"))?;
    let audio = has_audio_interface(&hid_dev).unwrap_or(false);
    let accessory = is_audio_accessory(&hid_dev).unwrap_or(false);
    // The interface that owns the HID function is the parent of `hid_dev`.
    let interface_number = hid_dev
        .parent()
        .and_then(|i| read_trimmed(&i.join("bInterfaceNumber")))
        .and_then(|s| u8::from_str_radix(&s, 16).ok());
    let report_descriptor_hex = std::fs::read(hid_dev.join("report_descriptor"))
        .ok()
        .map(|b| b.iter().map(|x| format!("{x:02x}")).collect());
    Some(HidrawNode {
        name: name.to_string(),
        vid,
        pid,
        manufacturer: read_trimmed(&usb_dev.join("manufacturer")),
        product: read_trimmed(&usb_dev.join("product")),
        interface_number,
        has_audio_interface: audio,
        is_audio_accessory: accessory,
        verdict: if audio || accessory {
            Verdict::Accessible
        } else {
            Verdict::NotAudioProduct
        },
        report_descriptor_hex,
    })
}

/// SteelSeries hidraw nodes found under `<sysfs_base>/class/hidraw`, sorted by
/// node number. A missing hidraw class directory yields an empty list.
pub fn scan(sysfs_base: &Path) -> Vec<HidrawNode> {
    let Ok(entries) = std::fs::read_dir(sysfs_base.join("class/hidraw")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("hidraw"))
        .collect();
    names.sort_by_key(|n| n.trim_start_matches("hidraw").parse::<u32>().unwrap_or(u32::MAX));
    names
        .iter()
        .filter_map(|n| describe(sysfs_base, n))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Mirrors real sysfs: usb_dev/<iface>/<hid_fn>, hidrawN/device -> hid_fn.
    fn add_node(dir: &TempDir, n: u32, usb: &str, vid: u16, pid: u16, audio: bool, iface: &str) {
        let usb_dev = dir.path().join(usb);
        let hid_fn = usb_dev.join(format!("iface{iface}/hid_fn{n}"));
        std::fs::create_dir_all(&hid_fn).unwrap();
        std::fs::write(usb_dev.join("idVendor"), format!("{vid:04x}\n")).unwrap();
        std::fs::write(usb_dev.join("idProduct"), format!("{pid:04x}\n")).unwrap();
        std::fs::write(usb_dev.join(format!("iface{iface}/bInterfaceNumber")), format!("{iface}\n")).unwrap();
        if audio {
            let a = usb_dev.join("audio_iface");
            std::fs::create_dir_all(&a).unwrap();
            std::fs::write(a.join("bInterfaceClass"), "01\n").unwrap();
        }
        let node = dir.path().join(format!("class/hidraw/hidraw{n}"));
        std::fs::create_dir_all(&node).unwrap();
        std::os::unix::fs::symlink(&hid_fn, node.join("device")).unwrap();
    }

    #[test]
    fn missing_class_dir_yields_nothing() {
        let dir = TempDir::new().unwrap();
        assert!(scan(dir.path()).is_empty());
    }

    #[test]
    fn unsupported_headset_is_reported_as_accessible() {
        let dir = TempDir::new().unwrap();
        add_node(&dir, 3, "usb_a", 0x1038, 0x2202, true, "05");
        let nodes = scan(dir.path());
        assert_eq!(nodes.len(), 1);
        let n = &nodes[0];
        assert_eq!((n.name.as_str(), n.vid, n.pid), ("hidraw3", 0x1038, 0x2202));
        assert_eq!(n.interface_number, Some(5));
        assert!(n.has_audio_interface);
        assert_eq!(n.verdict, Verdict::Accessible);
    }

    #[test]
    fn keyboard_is_reported_as_not_audio() {
        let dir = TempDir::new().unwrap();
        add_node(&dir, 1, "usb_k", 0x1038, 0x1234, false, "00");
        let nodes = scan(dir.path());
        assert_eq!(nodes[0].verdict, Verdict::NotAudioProduct);
        assert!(!nodes[0].has_audio_interface);
    }

    #[test]
    fn other_vendors_are_not_reported() {
        let dir = TempDir::new().unwrap();
        add_node(&dir, 0, "usb_x", 0x046d, 0xc52b, true, "00");
        assert!(scan(dir.path()).is_empty());
    }

    #[test]
    fn nodes_are_sorted_numerically() {
        let dir = TempDir::new().unwrap();
        add_node(&dir, 10, "usb_b", 0x1038, 0x2202, true, "00");
        add_node(&dir, 2, "usb_a", 0x1038, 0x2202, true, "01");
        let names: Vec<_> = scan(dir.path()).into_iter().map(|n| n.name).collect();
        assert_eq!(names, ["hidraw2", "hidraw10"]);
    }

    #[test]
    fn strings_and_descriptor_are_read_when_present() {
        let dir = TempDir::new().unwrap();
        add_node(&dir, 4, "usb_a", 0x1038, 0x2202, true, "00");
        std::fs::write(dir.path().join("usb_a/product"), "Arctis Nova X\n").unwrap();
        std::fs::write(dir.path().join("usb_a/manufacturer"), "SteelSeries\n").unwrap();
        std::fs::write(dir.path().join("usb_a/iface00/hid_fn4/report_descriptor"), [0x06, 0xc0, 0xff]).unwrap();
        let n = &scan(dir.path())[0];
        assert_eq!(n.product.as_deref(), Some("Arctis Nova X"));
        assert_eq!(n.manufacturer.as_deref(), Some("SteelSeries"));
        assert_eq!(n.report_descriptor_hex.as_deref(), Some("06c0ff"));
    }
}
