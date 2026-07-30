use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod api_executor;
pub mod builtins;
pub mod codec;
pub mod sync_dispatcher;
pub mod transform_eval;

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Parse {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    BaseNotFound {
        name: String,
    },
    Cycle(PathBuf),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Parse { path, source } => {
                write!(f, "YAML parse error in {}: {source}", path.display())
            }
            Self::BaseNotFound { name } => write!(f, "base file not found: {name}"),
            Self::Cycle(path) => write!(f, "extends cycle at {}", path.display()),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ── Field / struct types ──────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Uint8,
    Uint16,
    Uint32,
    Float32,
    ByteArray,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(default)]
    pub constant: Option<serde_yaml::Value>,
    /// Inclusive [min, max] range.
    #[serde(default)]
    pub range: Option<Vec<serde_yaml::Value>>,
    #[serde(default)]
    pub values: Option<Vec<serde_yaml::Value>>,
    #[serde(default)]
    pub repeat: Option<u32>,
    /// Required for `bytearray` fields.
    #[serde(default)]
    pub size: Option<u32>,
}

/// A struct field is either a concrete field definition or a `{struct: name}`
/// inline expansion (spec: `fields-from-struct`).
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum FieldOrRef {
    Ref {
        #[serde(rename = "struct")]
        struct_ref: String,
    },
    Field(FieldDef),
}

/// A struct is either flat (single list of fields) or bidirectional with
/// separate `outgoing` / `incoming` layouts.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum StructDef {
    Bidir {
        outgoing: Vec<FieldOrRef>,
        incoming: Vec<FieldOrRef>,
    },
    Flat(Vec<FieldOrRef>),
}

// ── API types ─────────────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub enum Transport {
    #[serde(rename = "HID_IO")]
    HidIo,
    #[serde(rename = "HID_FEATURE")]
    HidFeature,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ApiOp {
    pub transport: Transport,
    pub chunk_size: u32,
    #[serde(default)]
    pub payload_transform: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ApiDef {
    #[serde(default)]
    pub read: Option<ApiOp>,
    #[serde(default)]
    pub write: Option<ApiOp>,
}

// ── Transform types ───────────────────────────────────────────────────────────

/// Named value conversions. Keys and values in `CaseInt*::values` are stored as
/// raw YAML (`serde_yaml::Mapping`) so hex/integer keys are preserved faithfully
/// without requiring `Hash` on `serde_yaml::Value`.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransformDef {
    CaseIntToInt {
        #[serde(default)]
        default: Option<serde_yaml::Value>,
        values: serde_yaml::Mapping,
    },
    CaseIntToStr {
        #[serde(default)]
        default: Option<String>,
        values: serde_yaml::Mapping,
    },
    Linear {
        scale: f64,
        offset: f64,
    },
}

// ── Sync event types ──────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SyncEventField {
    pub name: String,
    pub byte: u8,
    #[serde(default)]
    pub transform: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SyncEventSideEffect {
    pub call: String,
    #[serde(default)]
    pub arg_byte: Option<u8>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct SyncEventDef {
    #[serde(default)]
    pub emit: Option<String>,
    #[serde(default)]
    pub fields: Option<Vec<SyncEventField>>,
    #[serde(default)]
    pub side_effects: Option<Vec<SyncEventSideEffect>>,
}

// ── Sync read types ───────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SyncReadMap {
    pub emit: String,
    /// Single source field (use `field` or `fields`, not both).
    #[serde(default)]
    pub field: Option<String>,
    /// Multiple source fields emitted as one event.
    #[serde(default)]
    pub fields: Option<Vec<String>>,
    #[serde(default)]
    pub transform: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SyncReadEntry {
    #[serde(rename = "struct")]
    pub struct_name: String,
    pub maps: Vec<SyncReadMap>,
}

// ── Lifecycle types ───────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct LifecycleCall {
    pub call: String,
    #[serde(default)]
    pub args: Option<serde_yaml::Value>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct Lifecycle {
    #[serde(default)]
    pub init: Option<Vec<LifecycleCall>>,
    #[serde(default)]
    pub post_init: Option<Vec<LifecycleCall>>,
    #[serde(default)]
    pub shutdown: Option<Vec<LifecycleCall>>,
}

// ── Device section types ──────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DeviceVariant {
    #[serde(default)]
    pub name: Option<String>,
    pub product_id: u16,
    #[serde(default)]
    pub bootloader_pid: Option<u16>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct HidInterface {
    pub interface: u8,
    #[serde(default)]
    pub alternate: Option<u8>,
    #[serde(default)]
    pub usage_page: Option<u16>,
    #[serde(default)]
    pub usage: Option<u16>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct HidConfig {
    #[serde(default)]
    pub usage_page: Option<u16>,
    #[serde(default)]
    pub usage: Option<u16>,
    #[serde(default)]
    pub command_interface: Option<HidInterface>,
    #[serde(default)]
    pub sync_interface: Option<HidInterface>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct DeviceSection {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub vendor_id: Option<u16>,
    #[serde(default)]
    pub variants: Option<Vec<DeviceVariant>>,
    #[serde(default)]
    pub hid: Option<HidConfig>,
    /// Free-form firmware version map (e.g. tx_dsp, required_engine, …).
    #[serde(default)]
    pub firmware: Option<serde_yaml::Value>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
}

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct DeviceConfig {
    /// Name of the base file to extend (without `.yaml`). Absent after merging.
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub constants: Option<HashMap<String, serde_yaml::Value>>,
    #[serde(default)]
    pub structs: Option<HashMap<String, StructDef>>,
    #[serde(default)]
    pub apis: Option<HashMap<String, ApiDef>>,
    #[serde(default)]
    pub transforms: Option<HashMap<String, TransformDef>>,
    /// Key is the command byte (e.g. `0xB5` in YAML → key 181).
    #[serde(default)]
    pub sync_events: Option<HashMap<u8, SyncEventDef>>,
    #[serde(default)]
    pub sync_read: Option<Vec<SyncReadEntry>>,
    #[serde(default)]
    pub lifecycle: Option<Lifecycle>,
    #[serde(default)]
    pub device: Option<DeviceSection>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load a device config file, recursively resolving `extends:` and merging.
///
/// `search_dirs` is the ordered list of directories searched when resolving
/// the base name from `extends:`.  Merge rules:
/// - Maps: overlay key wins on conflict.
/// - Lists (`sync_read`): overlay list replaces base list entirely.
/// - `lifecycle` hooks: each hook (`init`, `post_init`, `shutdown`) is
///   replaced independently — a missing overlay hook keeps the base hook.
/// - `device` section: overlay replaces base entirely.
pub fn load(path: &Path, search_dirs: &[&Path]) -> Result<DeviceConfig, LoadError> {
    let mut visited = std::collections::HashSet::new();
    load_inner(path, search_dirs, &mut visited)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn load_inner(
    path: &Path,
    search_dirs: &[&Path],
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<DeviceConfig, LoadError> {
    let canonical = path.canonicalize()?;
    if !visited.insert(canonical.clone()) {
        return Err(LoadError::Cycle(canonical));
    }

    let text = std::fs::read_to_string(path)?;
    let raw: DeviceConfig = serde_yaml::from_str(&text).map_err(|e| LoadError::Parse {
        path: path.to_owned(),
        source: e,
    })?;

    let result = if let Some(base_name) = raw.extends.clone() {
        let base_path = find_base(&base_name, search_dirs)
            .ok_or(LoadError::BaseNotFound { name: base_name })?;
        let base = load_inner(&base_path, search_dirs, visited)?;
        merge(base, raw)
    } else {
        raw
    };

    // Remove after successful load so a shared base (diamond) isn't flagged as a cycle.
    visited.remove(&canonical);
    Ok(result)
}

fn find_base(name: &str, search_dirs: &[&Path]) -> Option<PathBuf> {
    let filename = format!("{name}.yaml");
    for dir in search_dirs {
        let p = dir.join(&filename);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn merge(mut base: DeviceConfig, overlay: DeviceConfig) -> DeviceConfig {
    merge_map(&mut base.constants, overlay.constants);
    merge_map(&mut base.structs, overlay.structs);
    merge_map(&mut base.apis, overlay.apis);
    merge_map(&mut base.transforms, overlay.transforms);
    merge_map(&mut base.sync_events, overlay.sync_events);

    // Lists are replaced, not appended.
    if overlay.sync_read.is_some() {
        base.sync_read = overlay.sync_read;
    }

    // Each lifecycle hook is replaced independently.
    if let Some(o) = overlay.lifecycle {
        let b = base.lifecycle.get_or_insert_with(Lifecycle::default);
        if o.init.is_some() {
            b.init = o.init;
        }
        if o.post_init.is_some() {
            b.post_init = o.post_init;
        }
        if o.shutdown.is_some() {
            b.shutdown = o.shutdown;
        }
    }

    // Device section: overlay wins.
    if overlay.device.is_some() {
        base.device = overlay.device;
    }

    base.extends = None;
    base
}

fn merge_map<K, V>(base: &mut Option<HashMap<K, V>>, overlay: Option<HashMap<K, V>>)
where
    K: Eq + std::hash::Hash,
{
    match overlay {
        None => {}
        Some(o) => match base {
            None => *base = Some(o),
            Some(b) => b.extend(o),
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as _;

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn load_simple_config_no_extends() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "base.yaml",
            r#"
constants:
  report_id: 0x06
  timeout_ms: 50

structs:
  save_to_flash:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x09}

lifecycle:
  init:
    - call: sync_all
"#,
        );

        let cfg = load(&path, &[dir.path()]).unwrap();
        assert_eq!(cfg.extends, None);

        let constants = cfg.constants.unwrap();
        assert_eq!(constants.len(), 2);
        assert_eq!(constants["report_id"].as_u64(), Some(6));
        assert_eq!(constants["timeout_ms"].as_u64(), Some(50));

        assert!(cfg.structs.unwrap().contains_key("save_to_flash"));

        let lc = cfg.lifecycle.unwrap();
        assert_eq!(lc.init.unwrap().len(), 1);
    }

    #[test]
    fn extends_merges_base_sections() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "base.yaml",
            r#"
constants:
  report_id: 0x06

transforms:
  translate_battery:
    type: case_int_to_int
    default: 0
    values: {0: 0, 1: 50, 2: 100}

lifecycle:
  init:
    - call: sync_all
  shutdown:
    - call: save_to_flash
"#,
        );
        let device_path = write_file(
            dir.path(),
            "device.yaml",
            r#"
extends: base

device:
  name: "Test Device"
  vendor_id: 0x1038
  variants:
    - product_id: 0x12E0
  capabilities:
    - battery
"#,
        );

        let cfg = load(&device_path, &[dir.path()]).unwrap();

        assert_eq!(cfg.extends, None);
        assert!(cfg.constants.unwrap().contains_key("report_id"));
        assert!(cfg.transforms.unwrap().contains_key("translate_battery"));

        let lc = cfg.lifecycle.unwrap();
        assert_eq!(lc.init.unwrap().len(), 1);
        assert_eq!(lc.shutdown.unwrap().len(), 1);

        let dev = cfg.device.unwrap();
        assert_eq!(dev.name.as_deref(), Some("Test Device"));
        assert_eq!(dev.vendor_id, Some(0x1038));
        assert_eq!(dev.capabilities.unwrap(), vec!["battery"]);
    }

    #[test]
    fn overlay_constant_wins_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "base.yaml",
            "constants:\n  shared: 10\n  base_only: 1\n",
        );
        let device_path = write_file(
            dir.path(),
            "device.yaml",
            "extends: base\nconstants:\n  shared: 99\n  extra: 2\n",
        );

        let cfg = load(&device_path, &[dir.path()]).unwrap();
        let c = cfg.constants.unwrap();
        assert_eq!(c["shared"].as_u64(), Some(99));
        assert!(c.contains_key("base_only"));
        assert!(c.contains_key("extra"));
    }

    #[test]
    fn sync_read_list_replaced_not_appended() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "base.yaml",
            r#"
sync_read:
  - struct: audio_settings
    maps:
      - {emit: volume, field: vol}
  - struct: ux_settings
    maps:
      - {emit: brightness, field: bright}
"#,
        );
        let device_path = write_file(
            dir.path(),
            "device.yaml",
            r#"
extends: base

sync_read:
  - struct: wireless_settings
    maps:
      - {emit: battery, field: batt}
"#,
        );

        let cfg = load(&device_path, &[dir.path()]).unwrap();
        let sr = cfg.sync_read.unwrap();
        assert_eq!(sr.len(), 1, "list must be replaced, not appended");
        assert_eq!(sr[0].struct_name, "wireless_settings");
    }

    #[test]
    fn lifecycle_hooks_replaced_independently() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "base.yaml",
            r#"
lifecycle:
  init:
    - call: sync_all
  post_init:
    - call: send_battery_status
  shutdown:
    - call: save_to_flash
"#,
        );
        let device_path = write_file(
            dir.path(),
            "device.yaml",
            r#"
extends: base

lifecycle:
  init:
    - call: enable_sonar
    - call: sync_all
"#,
        );

        let cfg = load(&device_path, &[dir.path()]).unwrap();
        let lc = cfg.lifecycle.unwrap();
        assert_eq!(lc.init.unwrap().len(), 2, "overlay init replaces base init");
        assert_eq!(lc.post_init.unwrap().len(), 1, "base post_init preserved");
        assert_eq!(lc.shutdown.unwrap().len(), 1, "base shutdown preserved");
    }

    #[test]
    fn sync_events_parse_hex_byte_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "base.yaml",
            r#"
sync_events:
  0xB5:
    emit: radio_connection
    fields:
      - {name: status, byte: 4}
  0xB7:
    side_effects:
      - call: handle_battery
        arg_byte: 2
"#,
        );

        let cfg = load(&path, &[dir.path()]).unwrap();
        let se = cfg.sync_events.unwrap();
        assert!(se.contains_key(&0xB5u8));
        assert!(se.contains_key(&0xB7u8));
        assert_eq!(se[&0xB5u8].emit.as_deref(), Some("radio_connection"));
        assert_eq!(
            se[&0xB7u8].side_effects.as_ref().unwrap()[0].call,
            "handle_battery"
        );
    }

    #[test]
    fn bidir_struct_deserializes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "base.yaml",
            r#"
structs:
  battery_status:
    outgoing:
      - {name: report_id, type: uint8, constant: 0x06}
      - {name: command,   type: uint8, constant: 0xB7}
    incoming:
      - {name: report_id, type: uint8, constant: 0x06}
      - {name: level,     type: uint8, range: [0, 8]}
"#,
        );

        let cfg = load(&path, &[dir.path()]).unwrap();
        let s = &cfg.structs.unwrap()["battery_status"];
        let StructDef::Bidir { outgoing, incoming } = s else {
            panic!("expected Bidir, got Flat");
        };
        assert_eq!(outgoing.len(), 2);
        assert_eq!(incoming.len(), 2);
    }

    #[test]
    fn struct_ref_deserializes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "base.yaml",
            r#"
structs:
  custom_eq:
    - {name: report_id, type: uint8, constant: 0x06}
    - {struct: custom_eq_setting}
"#,
        );

        let cfg = load(&path, &[dir.path()]).unwrap();
        let StructDef::Flat(fields) = &cfg.structs.unwrap()["custom_eq"] else {
            panic!("expected Flat");
        };
        assert!(
            matches!(&fields[1], FieldOrRef::Ref { struct_ref } if struct_ref == "custom_eq_setting")
        );
    }

    #[test]
    fn cycle_detection_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.yaml", "extends: b\n");
        write_file(dir.path(), "b.yaml", "extends: a\n");

        let err = load(&dir.path().join("a.yaml"), &[dir.path()]).unwrap_err();
        assert!(matches!(err, LoadError::Cycle(_)));
    }

    #[test]
    fn base_not_found_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "device.yaml", "extends: nonexistent\n");

        let err = load(&path, &[dir.path()]).unwrap_err();
        assert!(matches!(err, LoadError::BaseNotFound { .. }));
    }
}
