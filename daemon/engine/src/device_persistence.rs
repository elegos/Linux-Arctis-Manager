use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

/// Path to the persisted device settings YAML file.
/// Format matches v2: `<base>/settings/<vid:04x>_<pid:04x>.yaml`.
pub fn settings_file_path(base_dir: &Path, vid: u16, pid: u16) -> PathBuf {
    base_dir
        .join("settings")
        .join(format!("{vid:04x}_{pid:04x}.yaml"))
}

pub fn save_device_settings(
    path: &Path,
    settings: &HashMap<String, JsonValue>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let yaml = serde_yaml::to_string(settings).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, yaml)
}

/// Load persisted device settings from a YAML file.
/// Returns an empty map when the file is absent or cannot be parsed.
pub fn load_device_settings(path: &Path) -> HashMap<String, JsonValue> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(&content) else {
        return HashMap::new();
    };
    map.iter()
        .filter_map(|(k, v)| {
            let key = k.as_str()?.to_owned();
            let val = yaml_value_to_json(v)?;
            Some((key, val))
        })
        .collect()
}

fn yaml_value_to_json(v: &serde_yaml::Value) -> Option<JsonValue> {
    match v {
        serde_yaml::Value::Null => Some(JsonValue::Null),
        serde_yaml::Value::Bool(b) => Some(JsonValue::Bool(*b)),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(JsonValue::from(i))
            } else {
                n.as_f64().map(JsonValue::from)
            }
        }
        serde_yaml::Value::String(s) => Some(JsonValue::from(s.as_str())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn settings_file_path_format() {
        let base = Path::new("/home/user/.config/arctis_manager");
        let p = settings_file_path(base, 0x1038, 0x12aa);
        assert_eq!(p, base.join("settings/1038_12aa.yaml"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = settings_file_path(dir.path(), 0x1038, 0x1234);
        let mut settings = HashMap::new();
        settings.insert("volume".to_owned(), JsonValue::from(50i64));
        settings.insert("sidetone".to_owned(), JsonValue::from(20i64));
        settings.insert("mic_led".to_owned(), JsonValue::Bool(true));
        save_device_settings(&path, &settings).unwrap();
        let loaded = load_device_settings(&path);
        assert_eq!(loaded["volume"], 50);
        assert_eq!(loaded["sidetone"], 20);
        assert_eq!(loaded["mic_led"], true);
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/sub/device.yaml");
        let settings = HashMap::new();
        assert!(save_device_settings(&path, &settings).is_ok());
        assert!(path.exists());
    }

    #[test]
    fn load_returns_empty_on_missing_file() {
        let loaded = load_device_settings(Path::new("/nonexistent/path/file.yaml"));
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_returns_empty_on_invalid_yaml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.yaml");
        std::fs::write(&path, b": invalid: yaml: [\n").unwrap();
        let loaded = load_device_settings(&path);
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_overwrites_same_key_on_save() {
        let dir = tempdir().unwrap();
        let path = settings_file_path(dir.path(), 0x1038, 0x1234);
        let mut settings = HashMap::new();
        settings.insert("volume".to_owned(), JsonValue::from(50i64));
        save_device_settings(&path, &settings).unwrap();
        settings.insert("volume".to_owned(), JsonValue::from(75i64));
        save_device_settings(&path, &settings).unwrap();
        let loaded = load_device_settings(&path);
        assert_eq!(loaded["volume"], 75);
    }
}
