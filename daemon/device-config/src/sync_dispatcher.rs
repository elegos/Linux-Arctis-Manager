use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use crate::codec::FieldValue;
use crate::transform_eval::{TransformError, TransformEval, TransformOutput};
use crate::{DeviceConfig, SyncEventDef};

// ── Event value ───────────────────────────────────────────────────────────────

/// A single field value carried in an emitted event.  Numeric fields keep their
/// `FieldValue` type; `case_int_to_str` transforms produce a labelled string.
#[derive(Debug, Clone, PartialEq)]
pub enum EventValue {
    Field(FieldValue),
    Str(String),
}

impl From<TransformOutput> for EventValue {
    fn from(o: TransformOutput) -> Self {
        match o {
            TransformOutput::Value(fv) => EventValue::Field(fv),
            TransformOutput::Str(s) => EventValue::Str(s),
        }
    }
}

impl From<FieldValue> for EventValue {
    fn from(fv: FieldValue) -> Self {
        EventValue::Field(fv)
    }
}

// ── Dispatch result ───────────────────────────────────────────────────────────

/// Named D-Bus signal to emit, with its extracted and optionally transformed fields.
#[derive(Debug)]
pub struct EmitEvent {
    pub signal: String,
    pub fields: HashMap<String, EventValue>,
}

/// An engine-internal call to invoke after field extraction.
#[derive(Debug, PartialEq)]
pub struct SideEffectCall {
    pub call: String,
    /// Byte value extracted from the report at `arg_byte`, if declared.
    pub arg: Option<u8>,
}

/// Full result of dispatching one sync report.
#[derive(Debug)]
pub struct DispatchResult {
    pub emit: Option<EmitEvent>,
    pub side_effects: Vec<SideEffectCall>,
}

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DispatchError {
    ReportTooShort {
        needed: usize,
        got: usize,
    },
    FieldOutOfRange {
        field: String,
        byte: u8,
        report_len: usize,
    },
    Transform(TransformError),
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReportTooShort { needed, got } => {
                write!(f, "report too short: need {needed} bytes, got {got}")
            }
            Self::FieldOutOfRange {
                field,
                byte,
                report_len,
            } => {
                write!(
                    f,
                    "field '{field}' at byte {byte} out of range (report is {report_len} bytes)"
                )
            }
            Self::Transform(e) => write!(f, "transform error: {e}"),
        }
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Transform(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

impl From<TransformError> for DispatchError {
    fn from(e: TransformError) -> Self {
        Self::Transform(e)
    }
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

static EMPTY_EVENTS: OnceLock<HashMap<u8, SyncEventDef>> = OnceLock::new();

pub struct SyncDispatcher<'a> {
    sync_events: &'a HashMap<u8, SyncEventDef>,
    transforms: TransformEval<'a>,
}

impl<'a> SyncDispatcher<'a> {
    pub fn new(cfg: &'a DeviceConfig) -> Self {
        Self {
            sync_events: cfg
                .sync_events
                .as_ref()
                .unwrap_or_else(|| EMPTY_EVENTS.get_or_init(HashMap::new)),
            transforms: TransformEval::new(cfg),
        }
    }

    /// Dispatch a raw HID sync report.
    ///
    /// Returns `Ok(None)` if the command byte (report[1]) has no entry in the
    /// `sync_events` table.  Returns `Ok(Some(…))` on a successful match,
    /// or `Err` if the report is malformed or a transform fails.
    pub fn dispatch(&self, report: &[u8]) -> Result<Option<DispatchResult>, DispatchError> {
        if report.len() < 2 {
            return Err(DispatchError::ReportTooShort {
                needed: 2,
                got: report.len(),
            });
        }
        let cmd = report[1];
        let entry = match self.sync_events.get(&cmd) {
            Some(e) => e,
            None => return Ok(None),
        };

        let emit = self.build_emit(report, entry)?;
        let side_effects = self.build_side_effects(report, entry)?;

        Ok(Some(DispatchResult { emit, side_effects }))
    }

    fn build_emit(
        &self,
        report: &[u8],
        entry: &SyncEventDef,
    ) -> Result<Option<EmitEvent>, DispatchError> {
        let signal = match &entry.emit {
            Some(s) => s.clone(),
            None => return Ok(None),
        };

        let mut fields = HashMap::new();
        for f in entry.fields.iter().flatten() {
            let byte_idx = f.byte as usize;
            if byte_idx >= report.len() {
                return Err(DispatchError::FieldOutOfRange {
                    field: f.name.clone(),
                    byte: f.byte,
                    report_len: report.len(),
                });
            }
            let raw = FieldValue::U8(report[byte_idx]);
            let value: EventValue = match &f.transform {
                Some(t) => self.transforms.apply(t, &raw)?.into(),
                None => raw.into(),
            };
            fields.insert(f.name.clone(), value);
        }

        Ok(Some(EmitEvent { signal, fields }))
    }

    fn build_side_effects(
        &self,
        report: &[u8],
        entry: &SyncEventDef,
    ) -> Result<Vec<SideEffectCall>, DispatchError> {
        let mut out = Vec::new();
        for se in entry.side_effects.iter().flatten() {
            let arg = match se.arg_byte {
                Some(b) => {
                    let idx = b as usize;
                    if idx >= report.len() {
                        return Err(DispatchError::FieldOutOfRange {
                            field: se.call.clone(),
                            byte: b,
                            report_len: report.len(),
                        });
                    }
                    Some(report[idx])
                }
                None => None,
            };
            out.push(SideEffectCall {
                call: se.call.clone(),
                arg,
            });
        }
        Ok(out)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(yaml: &str) -> DeviceConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn unknown_command_byte_returns_none() {
        let c = cfg(r#"
sync_events:
  0x27:
    emit: high_gain
    fields:
      - {name: enabled, byte: 2}
"#);
        let d = SyncDispatcher::new(&c);
        let result = d.dispatch(&[0x06, 0x2E, 0x01]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn report_too_short_error() {
        let c = cfg("sync_events: {}");
        let d = SyncDispatcher::new(&c);
        assert!(matches!(
            d.dispatch(&[0x06]).unwrap_err(),
            DispatchError::ReportTooShort { needed: 2, .. }
        ));
        assert!(matches!(
            d.dispatch(&[]).unwrap_err(),
            DispatchError::ReportTooShort { needed: 2, .. }
        ));
    }

    #[test]
    fn simple_field_extraction_no_transform() {
        let c = cfg(r#"
sync_events:
  0x2E:
    emit: selected_eq_preset
    fields:
      - {name: id, byte: 2}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0x2E, 0x03, 0x00];
        let result = d.dispatch(&report).unwrap().unwrap();
        let emit = result.emit.unwrap();
        assert_eq!(emit.signal, "selected_eq_preset");
        assert_eq!(emit.fields["id"], EventValue::Field(FieldValue::U8(3)));
        assert!(result.side_effects.is_empty());
    }

    #[test]
    fn multiple_fields_extracted() {
        let c = cfg(r#"
sync_events:
  0x45:
    emit: chatmix
    fields:
      - {name: game_attenuation, byte: 2}
      - {name: chat_attenuation, byte: 3}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0x45, 0x64, 0x32];
        let result = d.dispatch(&report).unwrap().unwrap();
        let emit = result.emit.unwrap();
        assert_eq!(
            emit.fields["game_attenuation"],
            EventValue::Field(FieldValue::U8(0x64))
        );
        assert_eq!(
            emit.fields["chat_attenuation"],
            EventValue::Field(FieldValue::U8(0x32))
        );
    }

    #[test]
    fn field_with_case_int_to_int_transform() {
        let c = cfg(r#"
transforms:
  translate_battery:
    type: case_int_to_int
    default: 0
    values: {0: 0, 1: 12, 4: 50, 8: 100}
sync_events:
  0xB7:
    emit: battery
    fields:
      - {name: level, byte: 2, transform: translate_battery}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0xB7, 0x04, 0x00];
        let result = d.dispatch(&report).unwrap().unwrap();
        let emit = result.emit.unwrap();
        assert_eq!(emit.fields["level"], EventValue::Field(FieldValue::U8(50)));
    }

    #[test]
    fn field_with_case_int_to_str_transform() {
        let c = cfg(r#"
transforms:
  translate_radio:
    type: case_int_to_str
    default: UNKNOWN
    values:
      4: PAIRED_CONNECTED
      8: PAIRED_DISCONNECTED
sync_events:
  0xB5:
    emit: radio_connection
    fields:
      - {name: radio_connection_status, byte: 4, transform: translate_radio}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0xB5, 0x00, 0x00, 0x04, 0x00];
        let result = d.dispatch(&report).unwrap().unwrap();
        let emit = result.emit.unwrap();
        assert_eq!(
            emit.fields["radio_connection_status"],
            EventValue::Str("PAIRED_CONNECTED".to_string())
        );
    }

    #[test]
    fn side_effects_only_entry() {
        let c = cfg(r#"
sync_events:
  0xB7:
    side_effects:
      - {call: handle_headset_battery_event, arg_byte: 2}
      - {call: handle_charger_battery_event, arg_byte: 3}
      - {call: handle_charging_event,        arg_byte: 4}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0xB7, 0x04, 0x01, 0x02, 0x00];
        let result = d.dispatch(&report).unwrap().unwrap();
        assert!(result.emit.is_none());
        assert_eq!(result.side_effects.len(), 3);
        assert_eq!(
            result.side_effects[0],
            SideEffectCall {
                call: "handle_headset_battery_event".to_string(),
                arg: Some(0x04)
            }
        );
        assert_eq!(
            result.side_effects[1],
            SideEffectCall {
                call: "handle_charger_battery_event".to_string(),
                arg: Some(0x01)
            }
        );
        assert_eq!(
            result.side_effects[2],
            SideEffectCall {
                call: "handle_charging_event".to_string(),
                arg: Some(0x02)
            }
        );
    }

    #[test]
    fn emit_and_side_effects_together() {
        let c = cfg(r#"
transforms:
  translate_radio:
    type: case_int_to_str
    default: UNKNOWN
    values: {4: PAIRED_CONNECTED}
sync_events:
  0xB5:
    emit: radio_connection
    fields:
      - {name: radio_connection_status, byte: 4, transform: translate_radio}
    side_effects:
      - {call: send_connection_status, arg_byte: 4}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0xB5, 0x00, 0x00, 0x04, 0x00];
        let result = d.dispatch(&report).unwrap().unwrap();
        assert!(result.emit.is_some());
        assert_eq!(result.side_effects.len(), 1);
        assert_eq!(result.side_effects[0].call, "send_connection_status");
        assert_eq!(result.side_effects[0].arg, Some(0x04));
    }

    #[test]
    fn field_out_of_range_error() {
        let c = cfg(r#"
sync_events:
  0x27:
    emit: high_gain
    fields:
      - {name: enabled, byte: 10}
"#);
        let d = SyncDispatcher::new(&c);
        let short_report = [0x06u8, 0x27, 0x01];
        assert!(matches!(
            d.dispatch(&short_report).unwrap_err(),
            DispatchError::FieldOutOfRange { byte: 10, .. }
        ));
    }

    #[test]
    fn transform_no_match_propagates_error() {
        let c = cfg(r#"
transforms:
  strict:
    type: case_int_to_int
    values: {1: 10}
sync_events:
  0x27:
    emit: test
    fields:
      - {name: val, byte: 2, transform: strict}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0x27, 0x99]; // 0x99 not in table, no default
        assert!(matches!(
            d.dispatch(&report).unwrap_err(),
            DispatchError::Transform(_)
        ));
    }

    #[test]
    fn side_effect_without_arg_byte() {
        let c = cfg(r#"
sync_events:
  0x27:
    side_effects:
      - {call: some_handler}
"#);
        let d = SyncDispatcher::new(&c);
        let report = [0x06u8, 0x27, 0x01];
        let result = d.dispatch(&report).unwrap().unwrap();
        assert_eq!(result.side_effects[0].arg, None);
    }
}
