use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use crate::codec::FieldValue;
use crate::sync_dispatcher::{EmitEvent, EventValue};
use crate::transform_eval::{TransformError, TransformEval};
use crate::{DeviceConfig, SyncReadEntry};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SyncReadError {
    MissingField { struct_name: String, field: String },
    Transform(TransformError),
}

impl fmt::Display for SyncReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField { struct_name, field } => {
                write!(
                    f,
                    "sync_read: field '{field}' missing in response for '{struct_name}'"
                )
            }
            Self::Transform(e) => write!(f, "sync_read transform error: {e}"),
        }
    }
}

impl std::error::Error for SyncReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Transform(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

impl From<TransformError> for SyncReadError {
    fn from(e: TransformError) -> Self {
        Self::Transform(e)
    }
}

// ── SyncReader ────────────────────────────────────────────────────────────────

static EMPTY_ENTRIES: OnceLock<Vec<SyncReadEntry>> = OnceLock::new();

pub struct SyncReader<'a> {
    entries: &'a [SyncReadEntry],
    transforms: TransformEval<'a>,
}

impl<'a> SyncReader<'a> {
    pub fn new(cfg: &'a DeviceConfig) -> Self {
        Self {
            entries: cfg
                .sync_read
                .as_deref()
                .unwrap_or_else(|| EMPTY_ENTRIES.get_or_init(Vec::new)),
            transforms: TransformEval::new(cfg),
        }
    }

    /// Ordered list of sync_read entries.  The engine iterates these, reads each
    /// struct via `ApiExecutor`, then calls `map_entry` with the decoded fields.
    pub fn entries(&self) -> &[SyncReadEntry] {
        self.entries
    }

    /// Map a decoded API response for one `SyncReadEntry` into D-Bus emit events.
    ///
    /// `fields` is the output of `ApiExecutor::parse_response` for the entry's
    /// struct.  Each `SyncReadMap` in the entry produces one `EmitEvent`.
    pub fn map_entry(
        &self,
        entry: &SyncReadEntry,
        fields: &HashMap<String, FieldValue>,
    ) -> Result<Vec<EmitEvent>, SyncReadError> {
        let mut events = Vec::new();
        for map in &entry.maps {
            let event_fields = if let Some(name) = &map.field {
                let fv = fields
                    .get(name)
                    .ok_or_else(|| SyncReadError::MissingField {
                        struct_name: entry.struct_name.clone(),
                        field: name.clone(),
                    })?;
                let ev = self.apply_transform(fv, &map.transform, &entry.struct_name)?;
                let mut m = HashMap::new();
                m.insert(name.clone(), ev);
                m
            } else if let Some(names) = &map.fields {
                let mut m = HashMap::new();
                for name in names {
                    let fv = fields
                        .get(name)
                        .ok_or_else(|| SyncReadError::MissingField {
                            struct_name: entry.struct_name.clone(),
                            field: name.clone(),
                        })?;
                    let ev = self.apply_transform(fv, &map.transform, &entry.struct_name)?;
                    m.insert(name.clone(), ev);
                }
                m
            } else {
                HashMap::new()
            };
            events.push(EmitEvent {
                signal: map.emit.clone(),
                fields: event_fields,
            });
        }
        Ok(events)
    }

    fn apply_transform(
        &self,
        fv: &FieldValue,
        transform: &Option<String>,
        _struct_name: &str,
    ) -> Result<EventValue, SyncReadError> {
        Ok(match transform {
            Some(t) => self.transforms.apply(t, fv)?.into(),
            None => EventValue::Field(fv.clone()),
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(yaml: &str) -> DeviceConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn fields(pairs: &[(&str, FieldValue)]) -> HashMap<String, FieldValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn entries_returns_in_order() {
        let c = cfg(r#"
sync_read:
  - struct: audio_settings
    maps: [{emit: high_gain, field: device_gain}]
  - struct: ux_settings
    maps: [{emit: dim_timer, field: dim_timer}]
"#);
        let r = SyncReader::new(&c);
        let names: Vec<&str> = r.entries().iter().map(|e| e.struct_name.as_str()).collect();
        assert_eq!(names, vec!["audio_settings", "ux_settings"]);
    }

    #[test]
    fn entries_empty_when_no_sync_read() {
        let c = cfg("{}");
        let r = SyncReader::new(&c);
        assert!(r.entries().is_empty());
    }

    #[test]
    fn single_field_no_transform() {
        let c = cfg(r#"
sync_read:
  - struct: audio_settings
    maps:
      - {emit: selected_eq_preset, field: eq_preset}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        let f = fields(&[("eq_preset", FieldValue::U8(3))]);
        let events = r.map_entry(entry, &f).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].signal, "selected_eq_preset");
        assert_eq!(
            events[0].fields["eq_preset"],
            EventValue::Field(FieldValue::U8(3))
        );
    }

    #[test]
    fn single_field_with_case_int_to_int_transform() {
        let c = cfg(r#"
transforms:
  translate_battery:
    type: case_int_to_int
    default: 0
    values: {0: 0, 4: 50, 8: 100}
sync_read:
  - struct: status
    maps:
      - {emit: battery, field: level, transform: translate_battery}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        let f = fields(&[("level", FieldValue::U8(4))]);
        let events = r.map_entry(entry, &f).unwrap();
        assert_eq!(
            events[0].fields["level"],
            EventValue::Field(FieldValue::U8(50))
        );
    }

    #[test]
    fn single_field_with_case_int_to_str_transform() {
        let c = cfg(r#"
transforms:
  translate_radio:
    type: case_int_to_str
    default: UNKNOWN
    values: {4: PAIRED_CONNECTED, 8: PAIRED_DISCONNECTED}
sync_read:
  - struct: wireless_settings
    maps:
      - {emit: radio_connection, field: radio_connection_status,
         transform: translate_radio}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        let f = fields(&[("radio_connection_status", FieldValue::U8(4))]);
        let events = r.map_entry(entry, &f).unwrap();
        assert_eq!(
            events[0].fields["radio_connection_status"],
            EventValue::Str("PAIRED_CONNECTED".to_string())
        );
    }

    #[test]
    fn multiple_fields_no_transform() {
        let c = cfg(r#"
sync_read:
  - struct: audio_settings
    maps:
      - {emit: chatmix, fields: [game_attenuation, chat_attenuation]}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        let f = fields(&[
            ("game_attenuation", FieldValue::U8(0x64)),
            ("chat_attenuation", FieldValue::U8(0x32)),
        ]);
        let events = r.map_entry(entry, &f).unwrap();
        assert_eq!(events[0].signal, "chatmix");
        assert_eq!(events[0].fields.len(), 2);
        assert_eq!(
            events[0].fields["game_attenuation"],
            EventValue::Field(FieldValue::U8(0x64))
        );
    }

    #[test]
    fn multiple_fields_with_transform_applied_per_field() {
        let c = cfg(r#"
transforms:
  gain_to_db:
    type: linear
    scale: 0.5
    offset: -10.0
sync_read:
  - struct: audio_settings
    maps:
      - {emit: custom_eq, fields: [gain1, gain2], transform: gain_to_db}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        // gain=20 → 20*0.5 - 10 = 0.0 dB
        let f = fields(&[("gain1", FieldValue::U8(20)), ("gain2", FieldValue::U8(20))]);
        let events = r.map_entry(entry, &f).unwrap();
        assert_eq!(events[0].signal, "custom_eq");
        let EventValue::Field(FieldValue::F32(v)) = events[0].fields["gain1"] else {
            panic!("expected F32");
        };
        assert!((v - 0.0).abs() < 1e-4);
    }

    #[test]
    fn multiple_maps_produce_multiple_events() {
        let c = cfg(r#"
sync_read:
  - struct: audio_settings
    maps:
      - {emit: mic_volume,  field: mic_volume}
      - {emit: sidetone,    field: sidetone}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        let f = fields(&[
            ("mic_volume", FieldValue::U8(10)),
            ("sidetone", FieldValue::U8(5)),
        ]);
        let events = r.map_entry(entry, &f).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].signal, "mic_volume");
        assert_eq!(events[1].signal, "sidetone");
    }

    #[test]
    fn missing_field_returns_error() {
        let c = cfg(r#"
sync_read:
  - struct: audio_settings
    maps:
      - {emit: mic_volume, field: mic_volume}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        let f = fields(&[]); // empty — field missing
        assert!(matches!(
            r.map_entry(entry, &f).unwrap_err(),
            SyncReadError::MissingField { field, .. } if field == "mic_volume"
        ));
    }

    #[test]
    fn transform_error_propagates() {
        let c = cfg(r#"
transforms:
  strict:
    type: case_int_to_int
    values: {1: 10}
sync_read:
  - struct: audio_settings
    maps:
      - {emit: something, field: val, transform: strict}
"#);
        let r = SyncReader::new(&c);
        let entry = &r.entries()[0];
        let f = fields(&[("val", FieldValue::U8(99))]); // 99 not in table, no default
        assert!(matches!(
            r.map_entry(entry, &f).unwrap_err(),
            SyncReadError::Transform(_)
        ));
    }
}
