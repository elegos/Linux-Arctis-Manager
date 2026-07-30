use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use serde_yaml::Value as Yaml;

use crate::codec::FieldValue;
use crate::{DeviceConfig, TransformDef};

// ── Output ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TransformOutput {
    Value(FieldValue),
    Str(String),
}

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum TransformError {
    UnknownTransform(String),
    /// No key matched `input` and no default is configured.
    NoMatch {
        transform: String,
        input: i64,
    },
    InvalidInput {
        transform: String,
        detail: String,
    },
}

impl fmt::Display for TransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTransform(s) => write!(f, "unknown transform: {s}"),
            Self::NoMatch { transform, input } => {
                write!(f, "transform '{transform}': no match for {input}")
            }
            Self::InvalidInput { transform, detail } => {
                write!(f, "transform '{transform}': invalid input — {detail}")
            }
        }
    }
}

impl std::error::Error for TransformError {}

// ── Evaluator ─────────────────────────────────────────────────────────────────

static EMPTY_TRANSFORMS: OnceLock<HashMap<String, TransformDef>> = OnceLock::new();
static EMPTY_CONSTS: OnceLock<HashMap<String, Yaml>> = OnceLock::new();

pub struct TransformEval<'a> {
    transforms: &'a HashMap<String, TransformDef>,
    constants: &'a HashMap<String, Yaml>,
}

impl<'a> TransformEval<'a> {
    pub fn new(cfg: &'a DeviceConfig) -> Self {
        Self {
            transforms: cfg
                .transforms
                .as_ref()
                .unwrap_or_else(|| EMPTY_TRANSFORMS.get_or_init(HashMap::new)),
            constants: cfg
                .constants
                .as_ref()
                .unwrap_or_else(|| EMPTY_CONSTS.get_or_init(HashMap::new)),
        }
    }

    /// Evaluate named transform against `input`, returning the mapped output.
    pub fn apply(&self, name: &str, input: &FieldValue) -> Result<TransformOutput, TransformError> {
        let tdef = self
            .transforms
            .get(name)
            .ok_or_else(|| TransformError::UnknownTransform(name.to_string()))?;
        match tdef {
            TransformDef::CaseIntToInt { default, values } => {
                self.case_int_to_int(name, input, default, values)
            }
            TransformDef::CaseIntToStr { default, values } => {
                self.case_int_to_str(name, input, default, values)
            }
            TransformDef::Linear { scale, offset } => self.linear(name, input, *scale, *offset),
        }
    }

    fn case_int_to_int(
        &self,
        name: &str,
        input: &FieldValue,
        default: &Option<Yaml>,
        values: &serde_yaml::Mapping,
    ) -> Result<TransformOutput, TransformError> {
        let key = input.as_u64().ok_or_else(|| TransformError::InvalidInput {
            transform: name.to_string(),
            detail: "integer FieldValue required".to_string(),
        })?;

        for (k, v) in values.iter() {
            if yaml_key_matches(k, key) {
                let n = v.as_u64().ok_or_else(|| TransformError::InvalidInput {
                    transform: name.to_string(),
                    detail: "non-integer value in case table".to_string(),
                })?;
                return Ok(TransformOutput::Value(u64_to_fv(n)));
            }
        }

        if let Some(def) = default {
            let resolved =
                resolve_const(def, self.constants).ok_or_else(|| TransformError::InvalidInput {
                    transform: name.to_string(),
                    detail: "default constant reference could not be resolved".to_string(),
                })?;
            let n = resolved
                .as_u64()
                .ok_or_else(|| TransformError::InvalidInput {
                    transform: name.to_string(),
                    detail: "default is not an integer".to_string(),
                })?;
            return Ok(TransformOutput::Value(u64_to_fv(n)));
        }

        Err(TransformError::NoMatch {
            transform: name.to_string(),
            input: key as i64,
        })
    }

    fn case_int_to_str(
        &self,
        name: &str,
        input: &FieldValue,
        default: &Option<String>,
        values: &serde_yaml::Mapping,
    ) -> Result<TransformOutput, TransformError> {
        let key = input.as_u64().ok_or_else(|| TransformError::InvalidInput {
            transform: name.to_string(),
            detail: "integer FieldValue required".to_string(),
        })?;

        for (k, v) in values.iter() {
            if yaml_key_matches(k, key) {
                let s = v.as_str().ok_or_else(|| TransformError::InvalidInput {
                    transform: name.to_string(),
                    detail: "non-string value in case table".to_string(),
                })?;
                return Ok(TransformOutput::Str(s.to_string()));
            }
        }

        if let Some(def) = default {
            return Ok(TransformOutput::Str(def.clone()));
        }

        Err(TransformError::NoMatch {
            transform: name.to_string(),
            input: key as i64,
        })
    }

    fn linear(
        &self,
        name: &str,
        input: &FieldValue,
        scale: f64,
        offset: f64,
    ) -> Result<TransformOutput, TransformError> {
        let v = input.as_f64().ok_or_else(|| TransformError::InvalidInput {
            transform: name.to_string(),
            detail: "numeric FieldValue required".to_string(),
        })?;
        Ok(TransformOutput::Value(FieldValue::F32(
            (v * scale + offset) as f32,
        )))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// True if YAML key `k` represents the integer `target`.
fn yaml_key_matches(k: &Yaml, target: u64) -> bool {
    k.as_u64() == Some(target)
}

/// Resolve a literal YAML value or a `$name` constant reference.
/// Returns `None` if the reference does not exist in `constants`.
fn resolve_const<'a>(v: &'a Yaml, constants: &'a HashMap<String, Yaml>) -> Option<&'a Yaml> {
    if let Yaml::String(s) = v {
        if let Some(name) = s.strip_prefix('$') {
            return constants.get(name);
        }
    }
    Some(v)
}

/// Pack a u64 into the smallest unsigned FieldValue that fits.
fn u64_to_fv(n: u64) -> FieldValue {
    if n <= 0xFF {
        FieldValue::U8(n as u8)
    } else if n <= 0xFFFF {
        FieldValue::U16(n as u16)
    } else {
        FieldValue::U32(n as u32)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(yaml: &str) -> DeviceConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    // ── case_int_to_int ───────────────────────────────────────────────────────

    #[test]
    fn case_int_to_int_match() {
        let c = cfg(r#"
transforms:
  battery:
    type: case_int_to_int
    values: {0: 0, 1: 12, 2: 25, 4: 50, 8: 100}
"#);
        let ev = TransformEval::new(&c);
        assert_eq!(
            ev.apply("battery", &FieldValue::U8(4)).unwrap(),
            TransformOutput::Value(FieldValue::U8(50))
        );
    }

    #[test]
    fn case_int_to_int_match_large_value() {
        let c = cfg(r#"
transforms:
  t:
    type: case_int_to_int
    values: {0: 0, 1: 1000}
"#);
        let ev = TransformEval::new(&c);
        assert_eq!(
            ev.apply("t", &FieldValue::U8(1)).unwrap(),
            TransformOutput::Value(FieldValue::U16(1000))
        );
    }

    #[test]
    fn case_int_to_int_default_used_on_miss() {
        let c = cfg(r#"
transforms:
  battery:
    type: case_int_to_int
    default: 0
    values: {1: 12, 2: 25}
"#);
        let ev = TransformEval::new(&c);
        assert_eq!(
            ev.apply("battery", &FieldValue::U8(99)).unwrap(),
            TransformOutput::Value(FieldValue::U8(0))
        );
    }

    #[test]
    fn case_int_to_int_no_match_no_default_error() {
        let c = cfg(r#"
transforms:
  battery:
    type: case_int_to_int
    values: {1: 12}
"#);
        let ev = TransformEval::new(&c);
        assert!(matches!(
            ev.apply("battery", &FieldValue::U8(5)).unwrap_err(),
            TransformError::NoMatch { .. }
        ));
    }

    #[test]
    fn case_int_to_int_const_default() {
        let c = cfg(r#"
constants:
  sentinel: 255
transforms:
  t:
    type: case_int_to_int
    default: $sentinel
    values: {1: 12}
"#);
        let ev = TransformEval::new(&c);
        assert_eq!(
            ev.apply("t", &FieldValue::U8(0)).unwrap(),
            TransformOutput::Value(FieldValue::U8(255))
        );
    }

    // ── case_int_to_str ───────────────────────────────────────────────────────

    #[test]
    fn case_int_to_str_match() {
        let c = cfg(r#"
transforms:
  radio_status:
    type: case_int_to_str
    values:
      1: NOT_PAIRED_NOT_SEARCHING
      2: NOT_PAIRED_SEARCHING
      4: PAIRED_CONNECTED
      8: PAIRED_DISCONNECTED
"#);
        let ev = TransformEval::new(&c);
        assert_eq!(
            ev.apply("radio_status", &FieldValue::U8(4)).unwrap(),
            TransformOutput::Str("PAIRED_CONNECTED".to_string())
        );
    }

    #[test]
    fn case_int_to_str_default_used_on_miss() {
        let c = cfg(r#"
transforms:
  radio_status:
    type: case_int_to_str
    default: UNKNOWN
    values: {1: CONNECTED}
"#);
        let ev = TransformEval::new(&c);
        assert_eq!(
            ev.apply("radio_status", &FieldValue::U8(99)).unwrap(),
            TransformOutput::Str("UNKNOWN".to_string())
        );
    }

    #[test]
    fn case_int_to_str_no_match_no_default_error() {
        let c = cfg(r#"
transforms:
  t:
    type: case_int_to_str
    values: {1: ONE}
"#);
        let ev = TransformEval::new(&c);
        assert!(matches!(
            ev.apply("t", &FieldValue::U8(0)).unwrap_err(),
            TransformError::NoMatch { .. }
        ));
    }

    // ── linear ────────────────────────────────────────────────────────────────

    #[test]
    fn linear_basic() {
        let c = cfg(r#"
transforms:
  volume:
    type: linear
    scale: 0.5
    offset: 0.0
"#);
        let ev = TransformEval::new(&c);
        let TransformOutput::Value(FieldValue::F32(v)) =
            ev.apply("volume", &FieldValue::U8(100)).unwrap()
        else {
            panic!("expected F32")
        };
        assert!((v - 50.0).abs() < 1e-4);
    }

    #[test]
    fn linear_with_offset() {
        let c = cfg(r#"
transforms:
  temp:
    type: linear
    scale: 1.0
    offset: -40.0
"#);
        let ev = TransformEval::new(&c);
        let TransformOutput::Value(FieldValue::F32(v)) =
            ev.apply("temp", &FieldValue::U8(60)).unwrap()
        else {
            panic!("expected F32")
        };
        assert!((v - 20.0).abs() < 1e-4);
    }

    // ── error paths ───────────────────────────────────────────────────────────

    #[test]
    fn unknown_transform_error() {
        let empty = DeviceConfig::default();
        let ev = TransformEval::new(&empty);
        assert!(matches!(
            ev.apply("ghost", &FieldValue::U8(0)).unwrap_err(),
            TransformError::UnknownTransform(_)
        ));
    }

    #[test]
    fn invalid_input_type_for_case_transform() {
        let c = cfg(r#"
transforms:
  t:
    type: case_int_to_int
    values: {1: 2}
"#);
        let ev = TransformEval::new(&c);
        assert!(matches!(
            ev.apply("t", &FieldValue::Bytes(vec![0x01])).unwrap_err(),
            TransformError::InvalidInput { .. }
        ));
    }

    #[test]
    fn invalid_input_type_for_linear() {
        let c = cfg(r#"
transforms:
  t:
    type: linear
    scale: 1.0
    offset: 0.0
"#);
        let ev = TransformEval::new(&c);
        assert!(matches!(
            ev.apply("t", &FieldValue::Bytes(vec![0xFF])).unwrap_err(),
            TransformError::InvalidInput { .. }
        ));
    }
}
