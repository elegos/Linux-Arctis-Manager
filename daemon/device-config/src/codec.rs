// Struct codec: encode `FieldValue` maps → bytes and decode bytes → `FieldValue` maps,
// following the struct definitions and constraints from a loaded `DeviceConfig`.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use crate::{DeviceConfig, FieldDef, FieldOrRef, FieldType, StructDef};
use serde_yaml::Value as Yaml;

// ── FieldValue ────────────────────────────────────────────────────────────────

/// Runtime representation of a single struct field value.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    U8(u8),
    U16(u16),
    U32(u32),
    F32(f32),
    Bytes(Vec<u8>),
    /// Produced when the field has `repeat: n > 1`.
    Array(Vec<FieldValue>),
}

impl FieldValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::U8(v) => Some(*v as f64),
            Self::U16(v) => Some(*v as f64),
            Self::U32(v) => Some(*v as f64),
            Self::F32(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U8(v) => Some(*v as u64),
            Self::U16(v) => Some(*v as u64),
            Self::U32(v) => Some(*v as u64),
            Self::F32(v) => Some(*v as u64),
            _ => None,
        }
    }
}

// ── CodecError ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CodecError {
    UnknownStruct(String),
    UnknownConstant(String),
    MissingField {
        struct_name: String,
        field_name: String,
    },
    ConstantMismatch {
        field: String,
        expected: u64,
        got: u64,
    },
    ConstraintViolation {
        field: String,
        detail: String,
    },
    BufferTooShort {
        needed: usize,
        available: usize,
    },
    /// `bytearray` field has no `size:` set.
    MissingSize(String),
    InvalidValue {
        field: String,
        detail: String,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownStruct(s) => write!(f, "unknown struct: {s}"),
            Self::UnknownConstant(s) => write!(f, "unknown constant: ${s}"),
            Self::MissingField {
                struct_name,
                field_name,
            } => {
                write!(
                    f,
                    "struct '{struct_name}': missing value for field '{field_name}'"
                )
            }
            Self::ConstantMismatch {
                field,
                expected,
                got,
            } => {
                write!(
                    f,
                    "field '{field}': expected constant {expected:#x}, got {got:#x}"
                )
            }
            Self::ConstraintViolation { field, detail } => {
                write!(f, "field '{field}' constraint violation: {detail}")
            }
            Self::BufferTooShort { needed, available } => {
                write!(f, "buffer too short: need {needed} bytes, have {available}")
            }
            Self::MissingSize(field) => {
                write!(f, "field '{field}' is bytearray but has no size:")
            }
            Self::InvalidValue { field, detail } => {
                write!(f, "field '{field}': {detail}")
            }
        }
    }
}

impl std::error::Error for CodecError {}

// ── Codec ─────────────────────────────────────────────────────────────────────

static EMPTY_STRUCTS: OnceLock<HashMap<String, StructDef>> = OnceLock::new();
static EMPTY_CONSTS: OnceLock<HashMap<String, Yaml>> = OnceLock::new();

/// Encodes and decodes struct field maps to/from raw bytes following the DSL
/// type definitions, constant constraints, range constraints, and field types.
pub struct Codec<'a> {
    structs: &'a HashMap<String, StructDef>,
    constants: &'a HashMap<String, Yaml>,
}

impl<'a> Codec<'a> {
    pub fn new(
        structs: &'a HashMap<String, StructDef>,
        constants: &'a HashMap<String, Yaml>,
    ) -> Self {
        Self { structs, constants }
    }

    /// Convenience constructor from a loaded `DeviceConfig`.
    pub fn from_config(cfg: &'a DeviceConfig) -> Self {
        Self {
            structs: cfg
                .structs
                .as_ref()
                .unwrap_or_else(|| EMPTY_STRUCTS.get_or_init(HashMap::new)),
            constants: cfg
                .constants
                .as_ref()
                .unwrap_or_else(|| EMPTY_CONSTS.get_or_init(HashMap::new)),
        }
    }

    /// Encode `values` into bytes using the outgoing layout (flat structs use
    /// the single layout).  Constant fields are filled automatically; the caller
    /// need not (and should not) include them in `values`.
    pub fn serialize(
        &self,
        struct_name: &str,
        values: &HashMap<String, FieldValue>,
    ) -> Result<Vec<u8>, CodecError> {
        let fields = self.layout_for_write(struct_name)?;
        let mut buf = Vec::new();
        self.encode_fields(fields, struct_name, values, &mut buf)?;
        Ok(buf)
    }

    /// Decode `bytes` into a field map using the incoming layout (flat structs
    /// use the single layout).  Constant fields are validated but excluded from
    /// the returned map.
    pub fn deserialize(
        &self,
        struct_name: &str,
        bytes: &[u8],
    ) -> Result<HashMap<String, FieldValue>, CodecError> {
        let fields = self.layout_for_read(struct_name)?;
        let mut cursor = 0usize;
        let mut out = HashMap::new();
        self.decode_fields(fields, bytes, &mut cursor, &mut out)?;
        Ok(out)
    }

    // ── Layout selection ──────────────────────────────────────────────────────

    fn layout_for_write(&self, struct_name: &str) -> Result<&[FieldOrRef], CodecError> {
        match self
            .structs
            .get(struct_name)
            .ok_or_else(|| CodecError::UnknownStruct(struct_name.to_string()))?
        {
            StructDef::Flat(f) => Ok(f.as_slice()),
            StructDef::Bidir { outgoing, .. } => Ok(outgoing.as_slice()),
        }
    }

    fn layout_for_read(&self, struct_name: &str) -> Result<&[FieldOrRef], CodecError> {
        match self
            .structs
            .get(struct_name)
            .ok_or_else(|| CodecError::UnknownStruct(struct_name.to_string()))?
        {
            StructDef::Flat(f) => Ok(f.as_slice()),
            StructDef::Bidir { incoming, .. } => Ok(incoming.as_slice()),
        }
    }

    // ── Encoding helpers ──────────────────────────────────────────────────────

    fn encode_fields(
        &self,
        fields: &[FieldOrRef],
        struct_name: &str,
        values: &HashMap<String, FieldValue>,
        buf: &mut Vec<u8>,
    ) -> Result<(), CodecError> {
        for item in fields {
            match item {
                FieldOrRef::Ref { struct_ref } => {
                    let inner = self.layout_for_write(struct_ref)?;
                    self.encode_fields(inner, struct_ref, values, buf)?;
                }
                FieldOrRef::Field(def) => self.encode_field(def, struct_name, values, buf)?,
            }
        }
        Ok(())
    }

    fn encode_field(
        &self,
        def: &FieldDef,
        struct_name: &str,
        values: &HashMap<String, FieldValue>,
        buf: &mut Vec<u8>,
    ) -> Result<(), CodecError> {
        let count = def.repeat.unwrap_or(1) as usize;

        if let Some(const_yaml) = &def.constant {
            let resolved = resolve_const(const_yaml, self.constants)?;
            let fv = yaml_to_fv(resolved, &def.field_type, &def.name)?;
            for _ in 0..count {
                write_fv(&fv, buf);
            }
        } else {
            let fv = values
                .get(&def.name)
                .ok_or_else(|| CodecError::MissingField {
                    struct_name: struct_name.to_string(),
                    field_name: def.name.clone(),
                })?;
            match (count, fv) {
                (1, _) => write_fv(fv, buf),
                (_, FieldValue::Array(arr)) => {
                    for elem in arr {
                        write_fv(elem, buf);
                    }
                }
                _ => {
                    for _ in 0..count {
                        write_fv(fv, buf);
                    }
                }
            }
        }
        Ok(())
    }

    // ── Decoding helpers ──────────────────────────────────────────────────────

    fn decode_fields(
        &self,
        fields: &[FieldOrRef],
        bytes: &[u8],
        cursor: &mut usize,
        out: &mut HashMap<String, FieldValue>,
    ) -> Result<(), CodecError> {
        for item in fields {
            match item {
                FieldOrRef::Ref { struct_ref } => {
                    let inner = self.layout_for_read(struct_ref)?;
                    self.decode_fields(inner, bytes, cursor, out)?;
                }
                FieldOrRef::Field(def) => self.decode_field(def, bytes, cursor, out)?,
            }
        }
        Ok(())
    }

    fn decode_field(
        &self,
        def: &FieldDef,
        bytes: &[u8],
        cursor: &mut usize,
        out: &mut HashMap<String, FieldValue>,
    ) -> Result<(), CodecError> {
        let count = def.repeat.unwrap_or(1) as usize;
        if count == 1 {
            let fv = read_fv(bytes, cursor, &def.field_type, def.size, &def.name)?;
            self.validate(def, &fv)?;
            if def.constant.is_none() {
                out.insert(def.name.clone(), fv);
            }
        } else {
            let mut arr = Vec::with_capacity(count);
            for _ in 0..count {
                arr.push(read_fv(
                    bytes,
                    cursor,
                    &def.field_type,
                    def.size,
                    &def.name,
                )?);
            }
            if def.constant.is_none() {
                out.insert(def.name.clone(), FieldValue::Array(arr));
            }
        }
        Ok(())
    }

    fn validate(&self, def: &FieldDef, fv: &FieldValue) -> Result<(), CodecError> {
        if let Some(const_yaml) = &def.constant {
            let expected = resolve_const(const_yaml, self.constants)?
                .as_u64()
                .ok_or_else(|| CodecError::InvalidValue {
                    field: def.name.clone(),
                    detail: "constant is not a u64".to_string(),
                })?;
            let got = fv.as_u64().ok_or_else(|| CodecError::InvalidValue {
                field: def.name.clone(),
                detail: "field value is not numeric".to_string(),
            })?;
            if expected != got {
                return Err(CodecError::ConstantMismatch {
                    field: def.name.clone(),
                    expected,
                    got,
                });
            }
        }

        if let Some(range) = &def.range {
            if range.len() >= 2 {
                let v = fv.as_f64().ok_or_else(|| CodecError::InvalidValue {
                    field: def.name.clone(),
                    detail: "range validation requires a numeric field".to_string(),
                })?;
                let min = resolve_const(&range[0], self.constants)?
                    .as_f64()
                    .ok_or_else(|| CodecError::InvalidValue {
                        field: def.name.clone(),
                        detail: "range min is not numeric".to_string(),
                    })?;
                let max = resolve_const(&range[1], self.constants)?
                    .as_f64()
                    .ok_or_else(|| CodecError::InvalidValue {
                        field: def.name.clone(),
                        detail: "range max is not numeric".to_string(),
                    })?;
                if v < min || v > max {
                    return Err(CodecError::ConstraintViolation {
                        field: def.name.clone(),
                        detail: format!("{v} is outside [{min}, {max}]"),
                    });
                }
            }
        }

        if let Some(allowed_yaml) = &def.values {
            let v = fv.as_f64().ok_or_else(|| CodecError::InvalidValue {
                field: def.name.clone(),
                detail: "values validation requires a numeric field".to_string(),
            })?;
            let allowed: Result<Vec<f64>, _> = allowed_yaml
                .iter()
                .map(|yv| {
                    resolve_const(yv, self.constants)?.as_f64().ok_or_else(|| {
                        CodecError::InvalidValue {
                            field: def.name.clone(),
                            detail: "allowed value is not numeric".to_string(),
                        }
                    })
                })
                .collect();
            if !allowed?.contains(&v) {
                return Err(CodecError::ConstraintViolation {
                    field: def.name.clone(),
                    detail: format!("{v} is not in the allowed-values list"),
                });
            }
        }

        Ok(())
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

fn resolve_const<'a>(
    v: &'a Yaml,
    constants: &'a HashMap<String, Yaml>,
) -> Result<&'a Yaml, CodecError> {
    if let Yaml::String(s) = v {
        if let Some(name) = s.strip_prefix('$') {
            return constants
                .get(name)
                .ok_or_else(|| CodecError::UnknownConstant(name.to_string()));
        }
    }
    Ok(v)
}

fn yaml_to_fv(v: &Yaml, ft: &FieldType, field_name: &str) -> Result<FieldValue, CodecError> {
    let not_int = || CodecError::InvalidValue {
        field: field_name.to_string(),
        detail: "expected an integer YAML value".to_string(),
    };
    match ft {
        FieldType::Uint8 => Ok(FieldValue::U8(v.as_u64().ok_or_else(not_int)? as u8)),
        FieldType::Uint16 => Ok(FieldValue::U16(v.as_u64().ok_or_else(not_int)? as u16)),
        FieldType::Uint32 => Ok(FieldValue::U32(v.as_u64().ok_or_else(not_int)? as u32)),
        FieldType::Float32 => {
            Ok(FieldValue::F32(
                v.as_f64().ok_or_else(|| CodecError::InvalidValue {
                    field: field_name.to_string(),
                    detail: "expected a numeric YAML value".to_string(),
                })? as f32,
            ))
        }
        FieldType::ByteArray => Err(CodecError::InvalidValue {
            field: field_name.to_string(),
            detail: "bytearray fields cannot be constants".to_string(),
        }),
    }
}

fn write_fv(fv: &FieldValue, buf: &mut Vec<u8>) {
    match fv {
        FieldValue::U8(v) => buf.push(*v),
        FieldValue::U16(v) => buf.extend_from_slice(&v.to_be_bytes()),
        FieldValue::U32(v) => buf.extend_from_slice(&v.to_be_bytes()),
        FieldValue::F32(v) => buf.extend_from_slice(&v.to_be_bytes()),
        FieldValue::Bytes(v) => buf.extend_from_slice(v),
        FieldValue::Array(arr) => arr.iter().for_each(|elem| write_fv(elem, buf)),
    }
}

fn read_fv(
    bytes: &[u8],
    cursor: &mut usize,
    ft: &FieldType,
    size: Option<u32>,
    field_name: &str,
) -> Result<FieldValue, CodecError> {
    let needed = field_byte_size(ft, size, field_name)?;
    let available = bytes.len().saturating_sub(*cursor);
    if available < needed {
        return Err(CodecError::BufferTooShort { needed, available });
    }
    let slice = &bytes[*cursor..*cursor + needed];
    *cursor += needed;
    Ok(match ft {
        FieldType::Uint8 => FieldValue::U8(slice[0]),
        FieldType::Uint16 => FieldValue::U16(u16::from_be_bytes(slice.try_into().unwrap())),
        FieldType::Uint32 => FieldValue::U32(u32::from_be_bytes(slice.try_into().unwrap())),
        FieldType::Float32 => FieldValue::F32(f32::from_be_bytes(slice.try_into().unwrap())),
        FieldType::ByteArray => FieldValue::Bytes(slice.to_vec()),
    })
}

fn field_byte_size(
    ft: &FieldType,
    size: Option<u32>,
    field_name: &str,
) -> Result<usize, CodecError> {
    match ft {
        FieldType::Uint8 => Ok(1),
        FieldType::Uint16 => Ok(2),
        FieldType::Uint32 => Ok(4),
        FieldType::Float32 => Ok(4),
        FieldType::ByteArray => size
            .map(|s| s as usize)
            .ok_or_else(|| CodecError::MissingSize(field_name.to_string())),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldDef, FieldOrRef, FieldType, StructDef};

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn num(n: u64) -> Yaml {
        Yaml::Number(serde_yaml::Number::from(n))
    }

    fn float(f: f64) -> Yaml {
        Yaml::Number(serde_yaml::Number::from(f))
    }

    fn field(name: &str, ft: FieldType) -> FieldOrRef {
        FieldOrRef::Field(FieldDef {
            name: name.to_string(),
            field_type: ft,
            constant: None,
            range: None,
            values: None,
            values_mapping: None,
            repeat: None,
            size: None,
        })
    }

    fn const_field(name: &str, ft: FieldType, val: u64) -> FieldOrRef {
        FieldOrRef::Field(FieldDef {
            name: name.to_string(),
            field_type: ft,
            constant: Some(num(val)),
            range: None,
            values: None,
            values_mapping: None,
            repeat: None,
            size: None,
        })
    }

    fn range_field(name: &str, ft: FieldType, min: f64, max: f64) -> FieldOrRef {
        FieldOrRef::Field(FieldDef {
            name: name.to_string(),
            field_type: ft,
            constant: None,
            range: Some(vec![float(min), float(max)]),
            values: None,
            values_mapping: None,
            repeat: None,
            size: None,
        })
    }

    fn values_field(name: &str, ft: FieldType, vals: &[u64]) -> FieldOrRef {
        FieldOrRef::Field(FieldDef {
            name: name.to_string(),
            field_type: ft,
            constant: None,
            range: None,
            values: Some(vals.iter().copied().map(num).collect()),
            values_mapping: None,
            repeat: None,
            size: None,
        })
    }

    fn repeat_field(name: &str, ft: FieldType, n: u32) -> FieldOrRef {
        FieldOrRef::Field(FieldDef {
            name: name.to_string(),
            field_type: ft,
            constant: None,
            range: None,
            values: None,
            values_mapping: None,
            repeat: Some(n),
            size: None,
        })
    }

    fn bytearray_field(name: &str, size: u32) -> FieldOrRef {
        FieldOrRef::Field(FieldDef {
            name: name.to_string(),
            field_type: FieldType::ByteArray,
            constant: None,
            range: None,
            values: None,
            values_mapping: None,
            repeat: None,
            size: Some(size),
        })
    }

    fn flat(fields: Vec<FieldOrRef>) -> StructDef {
        StructDef::Flat(fields)
    }

    fn single_struct(name: &str, fields: Vec<FieldOrRef>) -> HashMap<String, StructDef> {
        let mut m = HashMap::new();
        m.insert(name.to_string(), flat(fields));
        m
    }

    fn no_consts() -> &'static HashMap<String, Yaml> {
        static EMPTY: OnceLock<HashMap<String, Yaml>> = OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    // ── Serialize ─────────────────────────────────────────────────────────────

    #[test]
    fn serialize_constants_auto_filled() {
        let structs = single_struct(
            "cmd",
            vec![
                const_field("report_id", FieldType::Uint8, 0x06),
                const_field("command", FieldType::Uint8, 0x09),
            ],
        );
        let codec = Codec::new(&structs, no_consts());
        let bytes = codec.serialize("cmd", &HashMap::new()).unwrap();
        assert_eq!(bytes, [0x06, 0x09]);
    }

    #[test]
    fn serialize_caller_values() {
        let structs = single_struct(
            "set_gain",
            vec![
                const_field("report_id", FieldType::Uint8, 0x06),
                field("gain", FieldType::Uint8),
            ],
        );
        let codec = Codec::new(&structs, no_consts());
        let mut values = HashMap::new();
        values.insert("gain".to_string(), FieldValue::U8(0x75));
        assert_eq!(codec.serialize("set_gain", &values).unwrap(), [0x06, 0x75]);
    }

    #[test]
    fn serialize_uint16_big_endian() {
        let structs = single_struct("s", vec![field("v", FieldType::Uint16)]);
        let codec = Codec::new(&structs, no_consts());
        let mut values = HashMap::new();
        values.insert("v".to_string(), FieldValue::U16(0x1234));
        assert_eq!(codec.serialize("s", &values).unwrap(), [0x12, 0x34]);
    }

    #[test]
    fn serialize_float32() {
        let structs = single_struct("s", vec![field("gain", FieldType::Float32)]);
        let codec = Codec::new(&structs, no_consts());
        let mut values = HashMap::new();
        values.insert("gain".to_string(), FieldValue::F32(1.0f32));
        let bytes = codec.serialize("s", &values).unwrap();
        assert_eq!(bytes, 1.0f32.to_be_bytes());
    }

    #[test]
    fn serialize_missing_field_error() {
        let structs = single_struct("s", vec![field("x", FieldType::Uint8)]);
        let codec = Codec::new(&structs, no_consts());
        let err = codec.serialize("s", &HashMap::new()).unwrap_err();
        assert!(matches!(err, CodecError::MissingField { .. }));
    }

    // ── Deserialize ───────────────────────────────────────────────────────────

    #[test]
    fn deserialize_excludes_constant_fields() {
        let structs = single_struct(
            "battery",
            vec![
                const_field("report_id", FieldType::Uint8, 0x06),
                field("level", FieldType::Uint8),
                field("status", FieldType::Uint8),
            ],
        );
        let codec = Codec::new(&structs, no_consts());
        let result = codec.deserialize("battery", &[0x06, 50, 2]).unwrap();
        assert_eq!(result["level"], FieldValue::U8(50));
        assert_eq!(result["status"], FieldValue::U8(2));
        assert!(
            !result.contains_key("report_id"),
            "constants excluded from output"
        );
    }

    #[test]
    fn deserialize_uint16_big_endian() {
        let structs = single_struct("s", vec![field("v", FieldType::Uint16)]);
        let codec = Codec::new(&structs, no_consts());
        let result = codec.deserialize("s", &[0xAB, 0xCD]).unwrap();
        assert_eq!(result["v"], FieldValue::U16(0xABCD));
    }

    #[test]
    fn deserialize_buffer_too_short() {
        let structs = single_struct("s", vec![field("v", FieldType::Uint16)]);
        let codec = Codec::new(&structs, no_consts());
        let err = codec.deserialize("s", &[0xAB]).unwrap_err();
        assert!(matches!(err, CodecError::BufferTooShort { .. }));
    }

    // ── Bidir struct ──────────────────────────────────────────────────────────

    #[test]
    fn bidir_serialize_uses_outgoing_deserialize_uses_incoming() {
        let mut structs = HashMap::new();
        structs.insert(
            "cmd".to_string(),
            StructDef::Bidir {
                outgoing: vec![
                    const_field("report_id", FieldType::Uint8, 0x06),
                    const_field("command", FieldType::Uint8, 0x42),
                ],
                incoming: vec![
                    const_field("report_id", FieldType::Uint8, 0x06),
                    const_field("command", FieldType::Uint8, 0x42),
                    field("result", FieldType::Uint8),
                ],
            },
        );
        let codec = Codec::new(&structs, no_consts());

        // Outgoing: 2 bytes
        let tx = codec.serialize("cmd", &HashMap::new()).unwrap();
        assert_eq!(tx, [0x06, 0x42]);

        // Incoming: 3 bytes, result extracted
        let rx = codec.deserialize("cmd", &[0x06, 0x42, 99]).unwrap();
        assert_eq!(rx["result"], FieldValue::U8(99));
    }

    // ── Constraint validation ─────────────────────────────────────────────────

    #[test]
    fn constant_mismatch_rejected_on_read() {
        let structs = single_struct("s", vec![const_field("id", FieldType::Uint8, 0x06)]);
        let codec = Codec::new(&structs, no_consts());
        let err = codec.deserialize("s", &[0xFF]).unwrap_err();
        assert!(matches!(err, CodecError::ConstantMismatch { .. }));
    }

    #[test]
    fn range_constraint_enforced_on_read() {
        let structs = single_struct("s", vec![range_field("level", FieldType::Uint8, 0.0, 8.0)]);
        let codec = Codec::new(&structs, no_consts());
        // 9 is out of [0, 8]
        assert!(matches!(
            codec.deserialize("s", &[9]).unwrap_err(),
            CodecError::ConstraintViolation { .. }
        ));
        // 8 is OK
        assert_eq!(
            codec.deserialize("s", &[8]).unwrap()["level"],
            FieldValue::U8(8)
        );
    }

    #[test]
    fn values_constraint_enforced_on_read() {
        let structs = single_struct(
            "s",
            vec![values_field("status", FieldType::Uint8, &[1, 2, 4, 8])],
        );
        let codec = Codec::new(&structs, no_consts());
        // 3 is not in the list
        assert!(matches!(
            codec.deserialize("s", &[3]).unwrap_err(),
            CodecError::ConstraintViolation { .. }
        ));
        // 4 is OK
        assert_eq!(
            codec.deserialize("s", &[4]).unwrap()["status"],
            FieldValue::U8(4)
        );
    }

    // ── Nested struct reference ───────────────────────────────────────────────

    #[test]
    fn nested_struct_ref_expands_inline() {
        let mut structs = HashMap::new();
        structs.insert(
            "gains".to_string(),
            flat(vec![
                field("g1", FieldType::Uint8),
                field("g2", FieldType::Uint8),
            ]),
        );
        structs.insert(
            "set_eq".to_string(),
            flat(vec![
                const_field("cmd", FieldType::Uint8, 0x33),
                FieldOrRef::Ref {
                    struct_ref: "gains".to_string(),
                },
            ]),
        );
        let codec = Codec::new(&structs, no_consts());

        let mut values = HashMap::new();
        values.insert("g1".to_string(), FieldValue::U8(10));
        values.insert("g2".to_string(), FieldValue::U8(20));
        assert_eq!(codec.serialize("set_eq", &values).unwrap(), [0x33, 10, 20]);

        let result = codec.deserialize("set_eq", &[0x33, 5, 7]).unwrap();
        assert_eq!(result["g1"], FieldValue::U8(5));
        assert_eq!(result["g2"], FieldValue::U8(7));
    }

    // ── Repeat field ─────────────────────────────────────────────────────────

    #[test]
    fn repeat_field_deserializes_to_array() {
        let structs = single_struct("s", vec![repeat_field("data", FieldType::Uint8, 3)]);
        let codec = Codec::new(&structs, no_consts());
        let result = codec.deserialize("s", &[10, 20, 30]).unwrap();
        assert_eq!(
            result["data"],
            FieldValue::Array(vec![
                FieldValue::U8(10),
                FieldValue::U8(20),
                FieldValue::U8(30)
            ])
        );
    }

    #[test]
    fn repeat_field_serializes_array() {
        let structs = single_struct("s", vec![repeat_field("data", FieldType::Uint8, 3)]);
        let codec = Codec::new(&structs, no_consts());
        let mut values = HashMap::new();
        values.insert(
            "data".to_string(),
            FieldValue::Array(vec![
                FieldValue::U8(1),
                FieldValue::U8(2),
                FieldValue::U8(3),
            ]),
        );
        assert_eq!(codec.serialize("s", &values).unwrap(), [1, 2, 3]);
    }

    // ── Bytearray field ───────────────────────────────────────────────────────

    #[test]
    fn bytearray_field_roundtrip() {
        let structs = single_struct("s", vec![bytearray_field("payload", 4)]);
        let codec = Codec::new(&structs, no_consts());
        let mut values = HashMap::new();
        values.insert(
            "payload".to_string(),
            FieldValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        );
        let bytes = codec.serialize("s", &values).unwrap();
        assert_eq!(bytes, [0xDE, 0xAD, 0xBE, 0xEF]);
        let result = codec.deserialize("s", &[0x01, 0x02, 0x03, 0x04]).unwrap();
        assert_eq!(result["payload"], FieldValue::Bytes(vec![1, 2, 3, 4]));
    }

    // ── $constant reference in constant field ─────────────────────────────────

    #[test]
    fn dollar_constant_reference_resolved() {
        let mut structs = HashMap::new();
        structs.insert(
            "cmd".to_string(),
            flat(vec![FieldOrRef::Field(FieldDef {
                name: "report_id".to_string(),
                field_type: FieldType::Uint8,
                constant: Some(Yaml::String("$report_id".to_string())),
                range: None,
                values: None,
                values_mapping: None,
                repeat: None,
                size: None,
            })]),
        );
        let mut constants = HashMap::new();
        constants.insert("report_id".to_string(), num(6));
        let codec = Codec::new(&structs, &constants);

        // Serialize: constant filled from $report_id = 6
        assert_eq!(codec.serialize("cmd", &HashMap::new()).unwrap(), [0x06]);

        // Deserialize: 0x06 matches → constant excluded from output
        assert!(codec.deserialize("cmd", &[0x06]).unwrap().is_empty());

        // Deserialize: 0x07 ≠ 6 → ConstantMismatch
        assert!(matches!(
            codec.deserialize("cmd", &[0x07]).unwrap_err(),
            CodecError::ConstantMismatch { .. }
        ));
    }

    #[test]
    fn unknown_struct_error() {
        let structs = HashMap::new();
        let codec = Codec::new(&structs, no_consts());
        assert!(matches!(
            codec.serialize("nope", &HashMap::new()).unwrap_err(),
            CodecError::UnknownStruct(_)
        ));
    }
}
