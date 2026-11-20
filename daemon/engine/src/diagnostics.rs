// Device diagnostics for the GUI's troubleshooting page: which SteelSeries
// hidraw nodes the system exposes, whether the helper would open them, and
// whether a device config covers them. Read-only; shipped as JSON over D-Bus.

use device_config::DeviceConfig;
use lam_hidraw_helper::scan::{HidrawNode, Verdict};
use serde_json::{json, Value};
use std::sync::Arc;

/// How a node relates to the shipped device configs.
fn classify(node: &HidrawNode, configs: &[Arc<DeviceConfig>]) -> (&'static str, Option<String>) {
    if node.verdict == Verdict::NotAudioProduct {
        // Keyboards, mice, ...: not what this app manages, nothing to report upstream.
        return ("ignored", None);
    }
    let found = configs.iter().find_map(|c| {
        let d = c.device.as_ref()?;
        let v = d
            .variants
            .as_ref()?
            .iter()
            .find(|v| v.product_id == node.pid)?;
        Some(
            v.name
                .clone()
                .or_else(|| d.name.clone())
                .unwrap_or_default(),
        )
    });
    match found {
        Some(name) => ("supported", Some(name)),
        None => ("unsupported", None),
    }
}

pub fn build_report(nodes: &[HidrawNode], configs: &[Arc<DeviceConfig>]) -> Value {
    let devices: Vec<Value> = nodes
        .iter()
        .map(|n| {
            let (status, config_name) = classify(n, configs);
            json!({
                "node": n.name,
                "vendor_id": format!("{:04x}", n.vid),
                "product_id": format!("{:04x}", n.pid),
                "manufacturer": n.manufacturer,
                "product": n.product,
                "interface": n.interface_number,
                "usb_audio": n.has_audio_interface,
                "audio_accessory": n.is_audio_accessory,
                "status": status,
                "config_name": config_name,
                "report_descriptor": n.report_descriptor_hex,
            })
        })
        .collect();
    json!({ "version": 1, "devices": devices })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(pid: u16, verdict: Verdict) -> HidrawNode {
        HidrawNode {
            name: "hidraw1".into(),
            vid: 0x1038,
            pid,
            manufacturer: None,
            product: Some("Arctis".into()),
            interface_number: Some(4),
            has_audio_interface: verdict == Verdict::Accessible,
            is_audio_accessory: false,
            verdict,
            report_descriptor_hex: Some("06c0ff".into()),
        }
    }

    fn config(pid: u16) -> Arc<DeviceConfig> {
        let yaml = format!(
            "device:\n  name: Arctis Test\n  vendor_id: 0x1038\n  variants:\n    - name: Arctis Test Variant\n      product_id: {pid}\n"
        );
        Arc::new(serde_yaml::from_str(&yaml).expect("valid test config"))
    }

    #[test]
    fn configured_audio_device_is_supported() {
        let r = build_report(&[node(0x2202, Verdict::Accessible)], &[config(0x2202)]);
        assert_eq!(r["devices"][0]["status"], "supported");
        assert_eq!(r["devices"][0]["config_name"], "Arctis Test Variant");
        assert_eq!(r["devices"][0]["product_id"], "2202");
    }

    #[test]
    fn audio_device_without_config_is_unsupported() {
        let r = build_report(&[node(0x9999, Verdict::Accessible)], &[config(0x2202)]);
        assert_eq!(r["devices"][0]["status"], "unsupported");
        assert!(r["devices"][0]["config_name"].is_null());
        assert_eq!(r["devices"][0]["report_descriptor"], "06c0ff");
    }

    #[test]
    fn non_audio_device_is_ignored_even_if_a_pid_matches() {
        let r = build_report(&[node(0x2202, Verdict::NotAudioProduct)], &[config(0x2202)]);
        assert_eq!(r["devices"][0]["status"], "ignored");
    }

    #[test]
    fn no_nodes_gives_empty_list() {
        let r = build_report(&[], &[]);
        assert_eq!(r["devices"].as_array().unwrap().len(), 0);
    }
}
