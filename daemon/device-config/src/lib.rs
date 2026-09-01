use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod api_executor;
pub mod builtins;
pub mod codec;
pub mod lifecycle_executor;
pub mod sync_dispatcher;
pub mod sync_reader;
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
    /// Signed, two's-complement. Decodes/encodes with the same bit pattern
    /// as `Uint8`, just reinterpreted as negative above 0x7F — needed for
    /// wire fields the vendor spec declares as a plain signed byte (e.g. an
    /// EQ band's readback gain, ±0.1 dB units) rather than an offset/scaled
    /// unsigned value.
    Int8,
    /// See `Int8`; big-endian on the wire like `Uint16`.
    Int16,
    /// See `Int8`; big-endian on the wire like `Uint32`.
    Int32,
    Float32,
    ByteArray,
    /// Variable-length string, always the last field in its layout. On
    /// decode it consumes every remaining byte in the response and trims
    /// trailing NUL padding (matches the vendor spec's `varstring`, e.g. a
    /// free-text EQ preset name). On encode, `size` (if set) caps the byte
    /// length — writing a longer value is a `ConstraintViolation`, not a
    /// silent truncation.
    VarString,
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
    /// Optional integer→label map for settings widgets (e.g. `{"0":"Off","1":"On","2":"Auto"}`).
    /// When present, `field_to_schema` emits a `discrete_map` schema instead of a slider.
    #[serde(default)]
    pub values_mapping: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub repeat: Option<u32>,
    /// Required for `bytearray` fields (fixed size). Optional for
    /// `varstring` fields (max size on encode; ignored on decode, which
    /// always consumes to the end of the buffer).
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

/// One step of a multi-message write (`WriteApi::Sequence`): either a
/// physical HID write, or a pause before the next step. Mirrors the vendor
/// spec's `(chunk HIDIO ...)` / `(chunk HIDSLEEP ...)` chunk list, and the
/// pattern of several independent `api-write` calls sharing one logical
/// setting (e.g. a parametric EQ band write: name, then band data, then a
/// commit trigger).
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum WriteStep {
    Op {
        transport: Transport,
        chunk_size: u32,
        #[serde(default)]
        payload_transform: Option<String>,
    },
    Sleep {
        sleep_ms: u64,
    },
}

/// A write API is either the common single-message shorthand, or an
/// explicit ordered sequence of steps for devices whose protocol needs more
/// than one physical HID transaction per logical setting change.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum WriteApi {
    Single(ApiOp),
    Sequence { steps: Vec<WriteStep> },
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ApiDef {
    #[serde(default)]
    pub read: Option<ApiOp>,
    #[serde(default)]
    pub write: Option<WriteApi>,
    /// Marks this API as a one-shot fire-and-forget command (e.g. trigger
    /// RF/BT pairing, restore factory defaults) rather than a value-bearing
    /// setting. The engine itself dispatches an action's write exactly like
    /// any other `WriteApi` — this flag only matters to the D-Bus layer,
    /// which must never treat an action's non-constant fields (if any) as a
    /// persisted setting discoverable via `SetSetting`, and instead exposes
    /// actions through the dedicated `TriggerAction` method.
    #[serde(default)]
    pub action: bool,
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
    /// D-Bus display hint: `percentage`, `on_off`, `label`.  Overrides the raw
    /// Rust type string in the emitted status JSON so the GUI renders correctly.
    #[serde(default)]
    pub display_type: Option<String>,
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
    /// D-Bus display hint applied to all fields in this map: `percentage`, `on_off`, `label`.
    #[serde(default)]
    pub display_type: Option<String>,
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
    /// Maps category names to the ordered list of status field names to display.
    /// Absent on base configs that inherit the representation from a leaf config.
    #[serde(default)]
    pub representation: Option<HashMap<String, Vec<String>>>,
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

    // Representation: overlay wins.
    if overlay.representation.is_some() {
        base.representation = overlay.representation;
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
    fn representation_section_parses_and_merges() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "base.yaml",
            r#"
representation:
  headset:
    - headset_batt_level
    - charging_status
  wireless:
    - wireless_mode
"#,
        );
        let path = write_file(
            dir.path(),
            "device.yaml",
            r#"
extends: base

representation:
  headset:
    - headset_batt_level
  mic:
    - mic_volume
"#,
        );

        let cfg = load(&path, &[dir.path()]).unwrap();
        let rep = cfg.representation.expect("representation must be present");
        // overlay wins
        assert!(rep["headset"].contains(&"headset_batt_level".to_string()));
        assert!(!rep["headset"].contains(&"charging_status".to_string()));
        assert!(rep.contains_key("mic"));
        assert!(!rep.contains_key("wireless"));
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

#[cfg(test)]
mod nova_yaml_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn nova_pro_wireless_yaml_parses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("device-configs");
        let nova = dir.join("nova_pro_wireless.yaml");
        if !nova.exists() {
            return; // skip when not present (CI without device-configs)
        }
        let cfg = load(&nova, &[dir.as_path()]).expect("nova_pro_wireless.yaml must parse");
        let structs = cfg.structs.as_ref().expect("structs");
        let apis = cfg.apis.as_ref().expect("apis");
        let transforms = cfg.transforms.as_ref().expect("transforms");
        let sync_events = cfg.sync_events.as_ref().expect("sync_events");
        let sync_read = cfg.sync_read.as_ref().expect("sync_read");
        let device = cfg.device.as_ref().expect("device");

        // base structs
        assert!(structs.contains_key("save_to_flash"));
        assert!(structs.contains_key("audio_settings"));
        assert!(structs.contains_key("wireless_settings"));
        assert!(structs.contains_key("custom_eq"));

        // write APIs present
        assert!(apis.contains_key("high_gain"));
        assert!(apis.contains_key("custom_eq"));
        assert!(apis.contains_key("dim_timer"));

        // read APIs present
        assert!(apis.contains_key("audio_settings"));
        assert!(apis.contains_key("wireless_settings"));

        // transforms
        assert!(transforms.contains_key("gain_from_device"));
        assert!(transforms.contains_key("timer_enum_to_minutes"));
        assert!(transforms.contains_key("battery_level_to_percent"));

        // sync events
        assert!(sync_events.contains_key(&0x27));
        assert!(sync_events.contains_key(&0xB5));
        assert!(sync_events.contains_key(&0xB7));

        // sync_read entries
        let structs_read: Vec<&str> = sync_read.iter().map(|e| e.struct_name.as_str()).collect();
        assert!(structs_read.contains(&"audio_settings"));
        assert!(structs_read.contains(&"wireless_settings"));

        // device section
        let variants = device.variants.as_ref().expect("variants");
        assert!(variants.iter().any(|v| v.product_id == 0x12E0));
        assert_eq!(device.vendor_id, Some(0x1038));
    }

    /// End-to-end proof that Nova 7 Gen2's `get_eq_preset_data` readback —
    /// the whole point of [E7-S12]'s `int8` field type — decodes a real
    /// device response correctly through the actual loaded YAML (byte
    /// offsets, band stride, signed gain included), not just a synthetic
    /// struct built by hand in `codec.rs`'s own unit tests.
    #[test]
    fn nova7gen2_get_eq_preset_data_decodes_negative_gain() {
        use crate::api_executor::ApiExecutor;
        use crate::codec::FieldValue;

        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("device-configs");
        let nova = dir.join("nova_7_gen2.yaml");
        if !nova.exists() {
            return; // skip when not present (CI without device-configs)
        }
        let cfg = load(&nova, &[dir.as_path()]).expect("nova_7_gen2.yaml must parse");
        let api = ApiExecutor::new(&cfg);

        // Read request: report_id/command/connection_type, all constant,
        // padded to the 65-byte chunk.
        let read_op = api.prepare_read("get_eq_preset_data").unwrap();
        assert_eq!(&read_op.request_bytes[..3], [0x00, 0x32, 0x00]);
        assert_eq!(read_op.request_bytes.len(), 65);

        // Synthetic response: header (report_id, command, connection_type)
        // then 10 bands x 6 bytes (freq u16 BE, filter_type u8, gain i8,
        // q_factor u16 BE). Only band 1 carries interesting values; bands
        // 2-10 use a minimal in-range "no filter" band (frequency/q_factor
        // range minimums, filter_type/gain 0) — an all-zero band would
        // violate band*_frequency's [20, 20001] and band*_q_factor's
        // [200, 10000] range constraints.
        let mut resp = vec![0x00, 0x32, 0x00];
        resp.extend_from_slice(&[0x00, 0x64]); // band1_frequency = 100
        resp.push(0x01); // band1_filter_type = 1 (peak)
        resp.push(0xF4); // band1_gain = -12 (i8 two's complement) = -1.2 dB
        resp.extend_from_slice(&[0x09, 0xC4]); // band1_q_factor = 2500 (2.5 x1000)

        let empty_band = [0x00, 0x14, 0x00, 0x00, 0x00, 0xC8];
        for _ in 0..9 {
            resp.extend_from_slice(&empty_band);
        }

        let decoded = api.parse_response("get_eq_preset_data", &resp).unwrap();
        assert_eq!(decoded["band1_frequency"], FieldValue::U16(100));
        assert_eq!(decoded["band1_filter_type"], FieldValue::U8(1));
        assert_eq!(decoded["band1_gain"], FieldValue::I8(-12));
        assert_eq!(decoded["band1_q_factor"], FieldValue::U16(2500));
        assert_eq!(decoded["band10_gain"], FieldValue::I8(0));
    }

    /// Regression guard: every device file (anything not starting with
    /// `base_`) in `device-configs/` must parse without error. Catches DSL
    /// typos/schema mistakes in new device conversions without needing a
    /// dedicated test per device.
    #[test]
    fn every_device_config_parses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("device-configs");
        if !dir.exists() {
            return; // skip when not present (CI without device-configs)
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("read device-configs dir") {
            let path = entry.expect("dir entry").path();
            let is_base = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("base_"));
            if is_base || path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            load(&path, &[dir.as_path()])
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
            checked += 1;
        }
        assert!(checked > 0, "expected at least one device config to check");
    }

    /// Regression guard: for every `emit:` name that appears in both
    /// `sync_events:` and `sync_read:`, the sync_read field names must be a
    /// subset of the live sync_event's field names. Both describe the same
    /// logical event and are read by name (`sync_dispatcher`/`sync_reader`
    /// both use the field's declared name — not its struct position — as the
    /// emitted JSON key), so a name that exists under one but not the other
    /// silently produces two differently-shaped events for what a D-Bus
    /// client expects to be the same signal. sync_read is allowed to cover
    /// fewer fields than the live event (e.g. a startup snapshot omitting a
    /// detail only worth pushing live), just never a *different* name for
    /// the same field.
    #[test]
    fn sync_event_and_sync_read_field_names_agree() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("device-configs");
        if !dir.exists() {
            return; // skip when not present (CI without device-configs)
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("read device-configs dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let is_base = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("base_"));
            if is_base {
                continue; // only fully-merged device files carry both sections
            }
            let cfg = load(&path, &[dir.as_path()])
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
            checked += 1;

            let Some(sync_events) = cfg.sync_events.as_ref() else {
                continue;
            };
            let Some(sync_read) = cfg.sync_read.as_ref() else {
                continue;
            };

            // emit name -> set of field names, from the live sync_events table.
            let mut live_fields: HashMap<&str, std::collections::HashSet<&str>> = HashMap::new();
            for def in sync_events.values() {
                let (Some(emit), Some(fields)) = (def.emit.as_deref(), def.fields.as_ref()) else {
                    continue;
                };
                let names = live_fields.entry(emit).or_default();
                for f in fields {
                    names.insert(f.name.as_str());
                }
            }

            for entry in sync_read {
                for map in &entry.maps {
                    let Some(live) = live_fields.get(map.emit.as_str()) else {
                        continue; // no live counterpart for this emit; nothing to compare
                    };
                    let read_names: Vec<&str> = match (&map.field, &map.fields) {
                        (Some(f), _) => vec![f.as_str()],
                        (None, Some(fs)) => fs.iter().map(String::as_str).collect(),
                        (None, None) => vec![],
                    };
                    for name in read_names {
                        assert!(
                            live.contains(name),
                            "{}: sync_read emit '{}' references field '{}', but the live \
                             sync_events entry for '{}' has no field of that name (has: {:?}) \
                             — the two will emit differently-shaped events for the same signal",
                            path.display(),
                            map.emit,
                            name,
                            map.emit,
                            live
                        );
                    }
                }
            }
        }
        assert!(checked > 0, "expected at least one device config to check");
    }
}
