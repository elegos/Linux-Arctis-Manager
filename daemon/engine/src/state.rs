// Shared engine state: updated by device tasks, read by the D-Bus service.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use device_config::codec::FieldValue;
use device_config::sync_dispatcher::EventValue;
use device_config::DeviceConfig;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;

// ── AppState ──────────────────────────────────────────────────────────────────

pub struct AppState {
    pub configs: Vec<Arc<DeviceConfig>>,
    pub devices: HashMap<PathBuf, DeviceEntry>,
    /// Directories to search for device configs, in priority order (first = highest).
    pub config_dirs: Vec<PathBuf>,
}

pub struct DeviceEntry {
    pub config: Arc<DeviceConfig>,
    #[allow(dead_code)] // read by upcoming status/hotplug signal extensions
    pub pid: u16,
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub capabilities: Vec<String>,
    /// Flat map of field_name → `{"value": ..., "type": "..."}`.
    pub status: HashMap<String, JsonValue>,
    /// Channel for sending API-write commands from the D-Bus layer to the
    /// device task's event loop.
    pub cmd_tx: mpsc::Sender<DeviceCommand>,
}

// ── DeviceCommand ─────────────────────────────────────────────────────────────

pub enum DeviceCommand {
    WriteApi {
        api_name: String,
        values: HashMap<String, FieldValue>,
    },
}

// ── SignalEvent ───────────────────────────────────────────────────────────────

/// Cross-task notification: device tasks send these; the D-Bus emitter task
/// receives them and emits the corresponding signals on the bus.
#[derive(Clone, Debug)]
pub enum SignalEvent {
    StatusChanged,
    SettingsChanged {
        json: String,
    },
    DeviceConnected {
        pid: u16,
        name: String,
        capabilities: Vec<String>,
    },
    DeviceDisconnected {
        pid: u16,
    },
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert an `EventValue` into a `{"value": ..., "type": "..."}` JSON object.
/// `display_type` overrides the raw Rust type string when present (e.g. `"percentage"`,
/// `"on_off"`), so the GUI receives the hint it expects.
pub fn event_value_to_json(ev: &EventValue, display_type: Option<&str>) -> JsonValue {
    let mut j = match ev {
        EventValue::Field(FieldValue::U8(v)) => {
            serde_json::json!({"value": v, "type": "uint8"})
        }
        EventValue::Field(FieldValue::U16(v)) => {
            serde_json::json!({"value": v, "type": "uint16"})
        }
        EventValue::Field(FieldValue::U32(v)) => {
            serde_json::json!({"value": v, "type": "uint32"})
        }
        EventValue::Field(FieldValue::F32(v)) => {
            serde_json::json!({"value": v, "type": "float32"})
        }
        EventValue::Field(FieldValue::Bytes(v)) => {
            serde_json::json!({"value": v, "type": "bytes"})
        }
        EventValue::Field(FieldValue::Array(v)) => {
            let arr: Vec<JsonValue> = v
                .iter()
                .map(|fv| event_value_to_json(&EventValue::Field(fv.clone()), None))
                .collect();
            serde_json::json!({"value": arr, "type": "array"})
        }
        EventValue::Str(s) => {
            serde_json::json!({"value": s, "type": "label"})
        }
    };
    if let Some(dt) = display_type {
        j["type"] = JsonValue::String(dt.to_string());
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    #[test]
    fn settings_changed_roundtrips_on_broadcast() {
        let (tx, mut rx) = broadcast::channel(4);
        let payload = r#"{"general":{},"device":{},"settings_config":{}}"#;
        tx.send(SignalEvent::SettingsChanged {
            json: payload.to_string(),
        })
        .unwrap();
        let ev = rx.try_recv().unwrap();
        match ev {
            SignalEvent::SettingsChanged { json } => assert_eq!(json, payload),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_value_to_json_u8() {
        let j = event_value_to_json(&EventValue::Field(FieldValue::U8(42)), None);
        assert_eq!(j["value"], 42);
        assert_eq!(j["type"], "uint8");
    }

    #[test]
    fn event_value_to_json_str() {
        let j = event_value_to_json(&EventValue::Str("CONNECTED".to_string()), None);
        assert_eq!(j["value"], "CONNECTED");
        assert_eq!(j["type"], "label");
    }

    #[test]
    fn event_value_to_json_display_type_override() {
        let j = event_value_to_json(&EventValue::Field(FieldValue::U8(75)), Some("percentage"));
        assert_eq!(j["value"], 75);
        assert_eq!(j["type"], "percentage");
    }

    #[test]
    fn event_value_to_json_on_off_override() {
        let j = event_value_to_json(&EventValue::Field(FieldValue::U8(0)), Some("on_off"));
        assert_eq!(j["value"], 0);
        assert_eq!(j["type"], "on_off");
    }
}
