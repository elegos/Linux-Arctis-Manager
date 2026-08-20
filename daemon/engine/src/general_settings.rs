use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(default)]
    pub redirect_audio_on_connect: bool,
    #[serde(default)]
    pub redirect_audio_on_disconnect: bool,
    #[serde(default)]
    pub redirect_audio_on_disconnect_device: Option<String>,
}

impl GeneralSettings {
    pub fn load_from_file(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_yaml::from_str(&content).unwrap_or_default()
    }

    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, yaml)
    }

    pub fn to_json(&self) -> JsonValue {
        serde_json::json!({
            "redirect_audio_on_connect": self.redirect_audio_on_connect,
            "redirect_audio_on_disconnect": self.redirect_audio_on_disconnect,
            "redirect_audio_on_disconnect_device": self.redirect_audio_on_disconnect_device,
        })
    }

    pub fn settings_config_json() -> JsonValue {
        serde_json::json!({
            "redirect_audio_on_connect": {
                "type": "toggle",
                "default_value": false,
                "values": {"on": true, "off": false, "on_label": "on", "off_label": "off"}
            },
            "redirect_audio_on_disconnect": {
                "type": "toggle",
                "default_value": false,
                "values": {"on": true, "off": false, "on_label": "on", "off_label": "off"}
            },
            "redirect_audio_on_disconnect_device": {
                "type": "select",
                "default_value": null,
                "options_source": "pulse_audio_devices",
                "options_mapping": {"value": "id", "label": "description"}
            }
        })
    }

    pub fn is_general_field(name: &str) -> bool {
        matches!(
            name,
            "redirect_audio_on_connect"
                | "redirect_audio_on_disconnect"
                | "redirect_audio_on_disconnect_device"
        )
    }

    pub fn set_field(&mut self, name: &str, value: &str) -> bool {
        let Ok(json_val) = serde_json::from_str::<JsonValue>(value) else {
            return false;
        };
        match name {
            "redirect_audio_on_connect" => {
                if let Some(b) = json_val.as_bool() {
                    self.redirect_audio_on_connect = b;
                    true
                } else {
                    false
                }
            }
            "redirect_audio_on_disconnect" => {
                if let Some(b) = json_val.as_bool() {
                    self.redirect_audio_on_disconnect = b;
                    true
                } else {
                    false
                }
            }
            "redirect_audio_on_disconnect_device" => {
                if json_val.is_null() {
                    self.redirect_audio_on_disconnect_device = None;
                    true
                } else if let Some(s) = json_val.as_str() {
                    self.redirect_audio_on_disconnect_device = Some(s.to_owned());
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn default_all_false_and_none() {
        let g = GeneralSettings::default();
        assert!(!g.redirect_audio_on_connect);
        assert!(!g.redirect_audio_on_disconnect);
        assert!(g.redirect_audio_on_disconnect_device.is_none());
    }

    #[test]
    fn to_json_contains_all_fields() {
        let g = GeneralSettings {
            redirect_audio_on_connect: true,
            redirect_audio_on_disconnect: false,
            redirect_audio_on_disconnect_device: Some("alsa_output.pci-test".to_owned()),
        };
        let j = g.to_json();
        assert_eq!(j["redirect_audio_on_connect"], true);
        assert_eq!(j["redirect_audio_on_disconnect"], false);
        assert_eq!(
            j["redirect_audio_on_disconnect_device"],
            "alsa_output.pci-test"
        );
    }

    #[test]
    fn to_json_null_device_when_none() {
        let g = GeneralSettings::default();
        let j = g.to_json();
        assert!(j["redirect_audio_on_disconnect_device"].is_null());
    }

    #[test]
    fn settings_config_has_three_fields_with_correct_types() {
        let sc = GeneralSettings::settings_config_json();
        assert_eq!(sc["redirect_audio_on_connect"]["type"], "toggle");
        assert_eq!(sc["redirect_audio_on_disconnect"]["type"], "toggle");
        assert_eq!(sc["redirect_audio_on_disconnect_device"]["type"], "select");
        assert_eq!(
            sc["redirect_audio_on_disconnect_device"]["options_source"],
            "pulse_audio_devices"
        );
    }

    #[test]
    fn is_general_field_identifies_correct_names() {
        assert!(GeneralSettings::is_general_field(
            "redirect_audio_on_connect"
        ));
        assert!(GeneralSettings::is_general_field(
            "redirect_audio_on_disconnect"
        ));
        assert!(GeneralSettings::is_general_field(
            "redirect_audio_on_disconnect_device"
        ));
        assert!(!GeneralSettings::is_general_field("volume"));
        assert!(!GeneralSettings::is_general_field("unknown"));
    }

    #[test]
    fn set_field_toggle_on_off() {
        let mut g = GeneralSettings::default();
        assert!(g.set_field("redirect_audio_on_connect", "true"));
        assert!(g.redirect_audio_on_connect);
        assert!(g.set_field("redirect_audio_on_connect", "false"));
        assert!(!g.redirect_audio_on_connect);
    }

    #[test]
    fn set_field_device_string_and_null() {
        let mut g = GeneralSettings::default();
        assert!(g.set_field(
            "redirect_audio_on_disconnect_device",
            r#""alsa_output.pci-test""#
        ));
        assert_eq!(
            g.redirect_audio_on_disconnect_device.as_deref(),
            Some("alsa_output.pci-test")
        );
        assert!(g.set_field("redirect_audio_on_disconnect_device", "null"));
        assert!(g.redirect_audio_on_disconnect_device.is_none());
    }

    #[test]
    fn set_field_wrong_type_returns_false() {
        let mut g = GeneralSettings::default();
        assert!(!g.set_field("redirect_audio_on_connect", r#""not_a_bool""#));
        assert!(!g.set_field("redirect_audio_on_disconnect_device", "123"));
    }

    #[test]
    fn set_field_bad_json_returns_false() {
        let mut g = GeneralSettings::default();
        assert!(!g.set_field("redirect_audio_on_connect", "not_json"));
    }

    #[test]
    fn set_field_unknown_returns_false() {
        let mut g = GeneralSettings::default();
        assert!(!g.set_field("volume", "50"));
    }

    #[test]
    fn roundtrip_yaml_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("general_settings.yaml");
        let original = GeneralSettings {
            redirect_audio_on_connect: true,
            redirect_audio_on_disconnect: true,
            redirect_audio_on_disconnect_device: Some("alsa_output.test".to_owned()),
        };
        original.save_to_file(&path).unwrap();
        let loaded = GeneralSettings::load_from_file(&path);
        assert_eq!(loaded.redirect_audio_on_connect, true);
        assert_eq!(loaded.redirect_audio_on_disconnect, true);
        assert_eq!(
            loaded.redirect_audio_on_disconnect_device.as_deref(),
            Some("alsa_output.test")
        );
    }

    #[test]
    fn load_missing_file_returns_default() {
        let g = GeneralSettings::load_from_file(Path::new("/nonexistent/path/file.yaml"));
        assert!(!g.redirect_audio_on_connect);
        assert!(g.redirect_audio_on_disconnect_device.is_none());
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path: PathBuf = dir.path().join("subdir/nested/general_settings.yaml");
        let g = GeneralSettings::default();
        assert!(g.save_to_file(&path).is_ok());
        assert!(path.exists());
    }
}
