// D-Bus service: three interfaces matching the v2 bus API, all on bus name
// `name.giacomofurlan.ArctisManager.Next`.  State is shared with device tasks
// via `Arc<Mutex<AppState>>`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::state::SignalEvent as SE;
use device_config::codec::FieldValue;
use device_config::{DeviceConfig, FieldDef, FieldOrRef, FieldType, StructDef};
use serde_json::{Map, Value as JsonValue};
use tokio::sync::{broadcast, Mutex};
use tracing::{error, warn};
use zbus::object_server::SignalEmitter;
use zbus::{connection, interface};

use crate::state::{AppState, DeviceCommand, SignalEvent};

// ── D-Bus constants ───────────────────────────────────────────────────────────

const BUS_NAME: &str = "name.giacomofurlan.ArctisManager.Next";
const STATUS_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Status";
const SETTINGS_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Settings";
const CONFIG_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Config";

// ── Status interface ──────────────────────────────────────────────────────────

struct StatusInterface {
    state: Arc<Mutex<AppState>>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Status")]
impl StatusInterface {
    async fn get_status(&self) -> String {
        let state = self.state.lock().await;
        build_status_json(&state)
    }

    #[zbus(signal)]
    async fn status_changed(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_connected(
        emitter: &SignalEmitter<'_>,
        product_id: u16,
        name: &str,
        capabilities: Vec<String>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_disconnected(emitter: &SignalEmitter<'_>, product_id: u16) -> zbus::Result<()>;
}

// ── Settings interface ────────────────────────────────────────────────────────

struct SettingsInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Settings")]
impl SettingsInterface {
    async fn get_settings(&self) -> String {
        let state = self.state.lock().await;
        build_settings_json(&state)
    }

    async fn set_setting(&self, setting: &str, value: &str) -> bool {
        let (api_name, field_value) = {
            let state = self.state.lock().await;
            let Some(entry) = state.devices.values().next() else {
                return false;
            };
            let Some(api) = find_api_for_field(&entry.config, setting) else {
                warn!("SetSetting: no API writes field '{setting}'");
                return false;
            };
            let fv = match parse_setting_value(&entry.config, &api, setting, value) {
                Some(fv) => fv,
                None => {
                    warn!("SetSetting: could not parse value '{value}' for '{setting}'");
                    return false;
                }
            };
            (api, fv)
        };

        let sent = {
            let state = self.state.lock().await;
            let Some(entry) = state.devices.values().next() else {
                return false;
            };
            let mut values = HashMap::new();
            values.insert(setting.to_string(), field_value);
            entry
                .cmd_tx
                .send(DeviceCommand::WriteApi { api_name, values })
                .await
                .is_ok()
        };

        if sent {
            let json = {
                let s = self.state.lock().await;
                build_settings_json(&s)
            };
            let _ = self.signal_tx.send(SE::SettingsChanged { json });
        }

        sent
    }

    async fn get_version(&self) -> String {
        env!("LAM_VERSION").to_string()
    }

    async fn get_list_options(&self, _list_name: &str) -> String {
        "[]".to_string()
    }

    #[zbus(signal)]
    async fn settings_changed(emitter: &SignalEmitter<'_>, settings_json: &str)
        -> zbus::Result<()>;
}

// ── Config interface ──────────────────────────────────────────────────────────

struct ConfigInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Config")]
impl ConfigInterface {
    async fn reload_configs(&self) -> bool {
        let dirs = {
            let state = self.state.lock().await;
            state.config_dirs.clone()
        };
        let dir_refs: Vec<&std::path::Path> =
            dirs.iter().map(std::path::PathBuf::as_path).collect();
        let new_configs = crate::load_configs_from_dirs(&dir_refs);
        let json = {
            let mut state = self.state.lock().await;
            state.configs = new_configs;
            build_settings_json(&state)
        };
        let _ = self.signal_tx.send(SE::SettingsChanged { json });
        true
    }
}

// ── Service startup ───────────────────────────────────────────────────────────

/// Register all interfaces on the session bus and spawn a background task that
/// forwards `SignalEvent`s from device tasks to D-Bus signals.
///
/// Returns the `Connection` so the caller can keep it alive.
pub async fn start_dbus_service(
    state: Arc<Mutex<AppState>>,
    mut signal_rx: broadcast::Receiver<SignalEvent>,
    signal_tx: broadcast::Sender<SignalEvent>,
) -> zbus::Result<zbus::Connection> {
    let conn = connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(
            STATUS_PATH,
            StatusInterface {
                state: Arc::clone(&state),
            },
        )?
        .serve_at(
            SETTINGS_PATH,
            SettingsInterface {
                state: Arc::clone(&state),
                signal_tx: signal_tx.clone(),
            },
        )?
        .serve_at(
            CONFIG_PATH,
            ConfigInterface {
                state: Arc::clone(&state),
                signal_tx,
            },
        )?
        .build()
        .await?;

    let status_emitter = SignalEmitter::new(&conn, STATUS_PATH)?.into_owned();
    let settings_emitter = SignalEmitter::new(&conn, SETTINGS_PATH)?.into_owned();

    tokio::spawn(async move {
        loop {
            let event = match signal_rx.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("D-Bus signal emitter lagged, dropped {n} events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            match event {
                SignalEvent::StatusChanged => {
                    let json = {
                        let s = state.lock().await;
                        build_status_json(&s)
                    };
                    if let Err(e) = StatusInterface::status_changed(&status_emitter, &json).await {
                        error!("StatusChanged signal failed: {e}");
                    }
                }
                SignalEvent::SettingsChanged { json } => {
                    if let Err(e) =
                        SettingsInterface::settings_changed(&settings_emitter, &json).await
                    {
                        error!("SettingsChanged signal failed: {e}");
                    }
                }
                SignalEvent::DeviceConnected {
                    pid,
                    name,
                    capabilities,
                } => {
                    if let Err(e) =
                        StatusInterface::device_connected(&status_emitter, pid, &name, capabilities)
                            .await
                    {
                        error!("DeviceConnected signal failed: {e}");
                    }
                }
                SignalEvent::DeviceDisconnected { pid } => {
                    if let Err(e) = StatusInterface::device_disconnected(&status_emitter, pid).await
                    {
                        error!("DeviceDisconnected signal failed: {e}");
                    }
                }
            }
        }
    });

    Ok(conn)
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn build_status_json(state: &AppState) -> String {
    let fields: Map<String, JsonValue> = state
        .devices
        .values()
        .flat_map(|entry| entry.status.iter().map(|(k, v)| (k.clone(), v.clone())))
        .collect();

    if fields.is_empty() {
        return "{}".to_string();
    }

    // GUI expects {category: {field: {value, type}}}; group everything under "headset".
    let result = serde_json::json!({"headset": fields});
    serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string())
}

fn build_settings_json(state: &AppState) -> String {
    let mut result = serde_json::json!({
        "general": {},
        "device": {},
        "settings_config": {}
    });

    let Some(entry) = state.devices.values().next() else {
        return result.to_string();
    };

    let (Some(apis), Some(structs)) = (&entry.config.apis, &entry.config.structs) else {
        return serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
    };

    let mut device_map = Map::new();
    let mut config_map = Map::new();

    for (api_name, api_def) in apis {
        if api_def.write.is_none() {
            continue;
        }
        let Some(struct_def) = structs.get(api_name.as_str()) else {
            continue;
        };
        for fdef in outgoing_fields(struct_def, structs) {
            if fdef.constant.is_some() {
                continue;
            }
            let current = entry
                .status
                .get(&fdef.name)
                .and_then(|v| v.get("value"))
                .cloned();

            // device section: plain current value (no {value, type} wrapper)
            if let Some(val) = &current {
                device_map
                    .entry(fdef.name.clone())
                    .or_insert_with(|| val.clone());
            }

            config_map
                .entry(fdef.name.clone())
                .or_insert_with(|| field_to_schema(fdef, current.as_ref()));
        }
    }

    result["device"] = JsonValue::Object(device_map);
    result["settings_config"] = JsonValue::Object(config_map);

    serde_json::to_string(&result).unwrap_or_else(|_| result.to_string())
}

fn yaml_to_json(y: &serde_yaml::Value) -> JsonValue {
    if let Some(i) = y.as_i64() {
        JsonValue::from(i)
    } else if let Some(f) = y.as_f64() {
        JsonValue::from(f)
    } else if let Some(s) = y.as_str() {
        JsonValue::from(s)
    } else if let Some(b) = y.as_bool() {
        JsonValue::from(b)
    } else {
        JsonValue::Null
    }
}

/// Build a `ConfigSetting`-compatible schema for one writable field.
///
/// The GUI constructs `ConfigSetting(name=..., **schema)`, so every key here
/// becomes an attribute.  Required keys: `type` (SettingType value string) and
/// `default_value`.
fn field_to_schema(fdef: &FieldDef, current_val: Option<&JsonValue>) -> JsonValue {
    // default_value: prefer the live current value, fall back to range minimum.
    let default_val = current_val.cloned().unwrap_or_else(|| {
        fdef.range
            .as_ref()
            .and_then(|r| r.first())
            .map(yaml_to_json)
            .unwrap_or(JsonValue::from(0i64))
    });

    if let Some(range) = &fdef.range {
        if range.len() == 2 {
            let min_f = range[0].as_f64().unwrap_or(0.0);
            let max_f = range[1].as_f64().unwrap_or(1.0);
            if (max_f - min_f) <= 1.0 {
                // Boolean [0, 1] range → toggle
                return serde_json::json!({
                    "type": "toggle",
                    "default_value": default_val,
                    "values": {
                        "on": yaml_to_json(&range[1]),
                        "off": yaml_to_json(&range[0]),
                        "on_label": "on",
                        "off_label": "off"
                    }
                });
            } else {
                // Wider numeric range → slider
                return serde_json::json!({
                    "type": "slider",
                    "default_value": default_val,
                    "min": yaml_to_json(&range[0]),
                    "max": yaml_to_json(&range[1]),
                    "step": 1
                });
            }
        }
    }

    if let Some(values) = &fdef.values {
        let mapping: Map<String, JsonValue> = values
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_str().map(|s| (i.to_string(), JsonValue::from(s))))
            .collect();
        return serde_json::json!({
            "type": "discrete_map",
            "default_value": default_val,
            "values_mapping": JsonValue::Object(mapping)
        });
    }

    // Fallback: treat as an unconstrained slider
    serde_json::json!({
        "type": "slider",
        "default_value": default_val,
        "min": 0,
        "max": 255,
        "step": 1
    })
}

/// Return the "outgoing" (write-side) flat list of concrete `FieldDef`s for
/// a struct, resolving one level of `{struct: name}` references.
fn outgoing_fields<'a>(
    struct_def: &'a StructDef,
    all_structs: &'a HashMap<String, StructDef>,
) -> Vec<&'a FieldDef> {
    let field_list: &[FieldOrRef] = match struct_def {
        StructDef::Bidir { outgoing, .. } => outgoing,
        StructDef::Flat(f) => f,
    };
    resolve_fields(field_list, all_structs)
}

fn resolve_fields<'a>(
    fields: &'a [FieldOrRef],
    all_structs: &'a HashMap<String, StructDef>,
) -> Vec<&'a FieldDef> {
    let mut result = Vec::new();
    for fof in fields {
        match fof {
            FieldOrRef::Field(fd) => result.push(fd),
            FieldOrRef::Ref { struct_ref } => {
                if let Some(inner) = all_structs.get(struct_ref.as_str()) {
                    // Resolve one level; avoid infinite recursion by not calling recursively.
                    let inner_fields = match inner {
                        StructDef::Bidir { outgoing, .. } => outgoing.as_slice(),
                        StructDef::Flat(f) => f.as_slice(),
                    };
                    for fof2 in inner_fields {
                        if let FieldOrRef::Field(fd) = fof2 {
                            result.push(fd);
                        }
                    }
                }
            }
        }
    }
    result
}

// ── SetSetting helpers ────────────────────────────────────────────────────────

/// Find the name of the first API whose write struct contains a non-constant
/// field named `field_name`.
pub fn find_api_for_field(config: &DeviceConfig, field_name: &str) -> Option<String> {
    let apis = config.apis.as_ref()?;
    let structs = config.structs.as_ref()?;

    for (api_name, api_def) in apis {
        if api_def.write.is_none() {
            continue;
        }
        let Some(struct_def) = structs.get(api_name.as_str()) else {
            continue;
        };
        let fields = outgoing_fields(struct_def, structs);
        for fdef in fields {
            if fdef.constant.is_none() && fdef.name == field_name {
                return Some(api_name.clone());
            }
        }
    }
    None
}

/// Parse a JSON-encoded string value into a `FieldValue` matching the type
/// expected by `field_name` in `api_name`'s write struct.
fn parse_setting_value(
    config: &DeviceConfig,
    api_name: &str,
    field_name: &str,
    raw: &str,
) -> Option<FieldValue> {
    let structs = config.structs.as_ref()?;
    let struct_def = structs.get(api_name)?;
    let fields = outgoing_fields(struct_def, structs);
    let fdef = fields.into_iter().find(|f| f.name == field_name)?;

    let json_val: serde_json::Value = serde_json::from_str(raw).ok()?;

    match fdef.field_type {
        FieldType::Uint8 => Some(FieldValue::U8(json_val.as_u64()? as u8)),
        FieldType::Uint16 => Some(FieldValue::U16(json_val.as_u64()? as u16)),
        FieldType::Uint32 => Some(FieldValue::U32(json_val.as_u64()? as u32)),
        FieldType::Float32 => Some(FieldValue::F32(json_val.as_f64()? as f32)),
        FieldType::ByteArray => None, // byte-array fields are not settable via D-Bus
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_version_is_semver_like() {
        let v = env!("LAM_VERSION");
        assert!(!v.is_empty());
        assert!(
            v.split('.').count() >= 2,
            "'{v}' has fewer than 2 version components"
        );
    }

    #[test]
    fn settings_changed_event_carries_json() {
        use tokio::sync::broadcast;
        let (tx, mut rx) = broadcast::channel::<SignalEvent>(4);
        let payload = r#"{"general":{},"device":{"volume":50},"settings_config":{}}"#;
        tx.send(SignalEvent::SettingsChanged {
            json: payload.to_string(),
        })
        .unwrap();
        match rx.try_recv().unwrap() {
            SignalEvent::SettingsChanged { json } => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap();
                assert_eq!(v["device"]["volume"], 50);
            }
            _ => panic!("wrong variant"),
        }
    }
    use device_config::{ApiDef, ApiOp, FieldType, Transport};
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    fn dummy_entry_with_status(
        status: HashMap<String, JsonValue>,
        cmd_tx: mpsc::Sender<DeviceCommand>,
    ) -> crate::state::DeviceEntry {
        let config = DeviceConfig::default();
        crate::state::DeviceEntry {
            config: Arc::new(config),
            pid: 0,
            name: "test".to_string(),
            capabilities: vec![],
            status,
            cmd_tx,
        }
    }

    #[test]
    fn build_status_json_empty() {
        let state = AppState {
            configs: vec![],
            devices: HashMap::new(),
            config_dirs: vec![PathBuf::from("/tmp")],
        };
        assert_eq!(build_status_json(&state), "{}");
    }

    #[test]
    fn build_status_json_with_field() {
        let (tx, _rx) = mpsc::channel(1);
        let mut status = HashMap::new();
        status.insert(
            "battery".to_string(),
            serde_json::json!({"value": 80, "type": "uint8"}),
        );
        let mut devices = HashMap::new();
        devices.insert(
            PathBuf::from("/dev/hidraw0"),
            dummy_entry_with_status(status, tx),
        );
        let state = AppState {
            configs: vec![],
            devices,
            config_dirs: vec![PathBuf::from("/tmp")],
        };
        let json: JsonValue = serde_json::from_str(&build_status_json(&state)).unwrap();
        // Fields are nested under the "headset" category for GUI compatibility.
        assert_eq!(json["headset"]["battery"]["value"], 80);
        assert_eq!(json["headset"]["battery"]["type"], "uint8");
    }

    #[test]
    fn find_api_for_field_found() {
        use std::collections::HashMap as Map;

        let mut structs = Map::new();
        structs.insert(
            "set_vol".to_string(),
            StructDef::Flat(vec![FieldOrRef::Field(FieldDef {
                name: "volume".to_string(),
                field_type: FieldType::Uint8,
                constant: None,
                range: None,
                values: None,
                repeat: None,
                size: None,
            })]),
        );
        let mut apis = Map::new();
        apis.insert(
            "set_vol".to_string(),
            ApiDef {
                read: None,
                write: Some(ApiOp {
                    transport: Transport::HidIo,
                    chunk_size: 64,
                    payload_transform: None,
                }),
            },
        );
        let config = DeviceConfig {
            structs: Some(structs),
            apis: Some(apis),
            ..DeviceConfig::default()
        };

        assert_eq!(
            find_api_for_field(&config, "volume"),
            Some("set_vol".to_string())
        );
        assert_eq!(find_api_for_field(&config, "nonexistent"), None);
    }
}
