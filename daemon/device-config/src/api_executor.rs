// API execution layer: translates named API calls into padded byte payloads
// (and parses responses back).  No I/O lives here — the engine drives the
// actual transport after calling prepare_write / prepare_read / parse_response.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use crate::codec::{Codec, CodecError, FieldValue};
use crate::{ApiDef, ApiOp, DeviceConfig, Transport};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ApiError {
    UnknownApi(String),
    NoWriteOp(String),
    NoReadOp(String),
    Codec(CodecError),
    UnknownTransform(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownApi(s) => write!(f, "unknown API: {s}"),
            Self::NoWriteOp(s) => write!(f, "API '{s}' has no write operation"),
            Self::NoReadOp(s) => write!(f, "API '{s}' has no read operation"),
            Self::Codec(e) => write!(f, "codec error: {e}"),
            Self::UnknownTransform(s) => write!(f, "unregistered builtin transform: {s}"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Codec(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

impl From<CodecError> for ApiError {
    fn from(e: CodecError) -> Self {
        Self::Codec(e)
    }
}

// ── Pending operations ────────────────────────────────────────────────────────

/// Serialised and padded bytes ready to transmit for a write API call.
#[derive(Debug)]
pub struct WriteOp {
    pub bytes: Vec<u8>,
    pub transport: Transport,
}

/// Serialised request bytes and metadata for a read API call.
/// Feed `request_bytes` to the transport, then pass the response bytes to
/// `ApiExecutor::parse_response`.
#[derive(Debug)]
pub struct ReadOp {
    pub request_bytes: Vec<u8>,
    pub transport: Transport,
    /// Maximum response size; allocate at least this many bytes before reading.
    pub chunk_size: usize,
}

// ── ApiExecutor ───────────────────────────────────────────────────────────────

pub type BuiltinFn = Box<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

static EMPTY_APIS: OnceLock<HashMap<String, ApiDef>> = OnceLock::new();

/// Prepares API call payloads and parses responses using the config's struct
/// definitions, constants, and transport/chunk-size metadata.
pub struct ApiExecutor<'a> {
    codec: Codec<'a>,
    apis: &'a HashMap<String, ApiDef>,
    builtins: HashMap<String, BuiltinFn>,
}

impl<'a> ApiExecutor<'a> {
    pub fn new(cfg: &'a DeviceConfig) -> Self {
        Self {
            codec: Codec::from_config(cfg),
            apis: cfg
                .apis
                .as_ref()
                .unwrap_or_else(|| EMPTY_APIS.get_or_init(HashMap::new)),
            builtins: HashMap::new(),
        }
    }

    /// Register a builtin payload transform referenced in the DSL as
    /// `payload_transform: builtin:<name>`.  Must be called before any
    /// `prepare_write` that uses the named transform.
    pub fn register_builtin(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&[u8]) -> Vec<u8> + Send + Sync + 'static,
    ) {
        self.builtins.insert(name.into(), Box::new(f));
    }

    /// Serialise `values`, apply any `payload_transform`, and pad to
    /// `chunk_size`.  Constant fields are filled automatically.
    pub fn prepare_write(
        &self,
        api_name: &str,
        values: &HashMap<String, FieldValue>,
    ) -> Result<WriteOp, ApiError> {
        let op = self.write_op(api_name)?;
        let mut bytes = self.codec.serialize(api_name, values)?;
        self.apply_transform(&mut bytes, &op.payload_transform)?;
        pad(&mut bytes, op.chunk_size as usize);
        Ok(WriteOp {
            bytes,
            transport: op.transport.clone(),
        })
    }

    /// Build the padded request bytes for a read API call.
    /// The outgoing layout is serialised with its constant fields filled in.
    pub fn prepare_read(&self, api_name: &str) -> Result<ReadOp, ApiError> {
        let op = self.read_op(api_name)?;
        let mut bytes = self.codec.serialize(api_name, &HashMap::new())?;
        pad(&mut bytes, op.chunk_size as usize);
        Ok(ReadOp {
            request_bytes: bytes,
            transport: op.transport.clone(),
            chunk_size: op.chunk_size as usize,
        })
    }

    /// Decode the device's response bytes for a read API, using the incoming
    /// struct layout.  Trailing padding bytes beyond the struct are ignored.
    pub fn parse_response(
        &self,
        api_name: &str,
        bytes: &[u8],
    ) -> Result<HashMap<String, FieldValue>, ApiError> {
        let _ = self.read_op(api_name)?; // validate existence before decode
        Ok(self.codec.deserialize(api_name, bytes)?)
    }

    // ── Private ───────────────────────────────────────────────────────────────

    fn write_op(&self, api_name: &str) -> Result<&ApiOp, ApiError> {
        self.apis
            .get(api_name)
            .ok_or_else(|| ApiError::UnknownApi(api_name.to_string()))?
            .write
            .as_ref()
            .ok_or_else(|| ApiError::NoWriteOp(api_name.to_string()))
    }

    fn read_op(&self, api_name: &str) -> Result<&ApiOp, ApiError> {
        self.apis
            .get(api_name)
            .ok_or_else(|| ApiError::UnknownApi(api_name.to_string()))?
            .read
            .as_ref()
            .ok_or_else(|| ApiError::NoReadOp(api_name.to_string()))
    }

    fn apply_transform(&self, bytes: &mut Vec<u8>, name: &Option<String>) -> Result<(), ApiError> {
        if let Some(n) = name {
            let f = self
                .builtins
                .get(n.as_str())
                .ok_or_else(|| ApiError::UnknownTransform(n.clone()))?;
            *bytes = f(bytes);
        }
        Ok(())
    }
}

fn pad(bytes: &mut Vec<u8>, size: usize) {
    if bytes.len() < size {
        bytes.resize(size, 0);
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
    fn prepare_write_fills_constants_and_pads() {
        let c = cfg(r#"
structs:
  save_to_flash:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x09}
apis:
  save_to_flash:
    write: {transport: HID_IO, chunk_size: 8}
"#);
        let exec = ApiExecutor::new(&c);
        let op = exec
            .prepare_write("save_to_flash", &HashMap::new())
            .unwrap();
        assert_eq!(op.bytes.len(), 8);
        assert_eq!(&op.bytes[..2], [0x06, 0x09]);
        assert_eq!(&op.bytes[2..], [0u8; 6]);
        assert_eq!(op.transport, Transport::HidIo);
    }

    #[test]
    fn prepare_write_includes_caller_values() {
        let c = cfg(r#"
structs:
  set_gain:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: gain,      type: uint8}
apis:
  set_gain:
    write: {transport: HID_IO, chunk_size: 4}
"#);
        let exec = ApiExecutor::new(&c);
        let mut values = HashMap::new();
        values.insert("gain".to_string(), FieldValue::U8(0x42));
        let op = exec.prepare_write("set_gain", &values).unwrap();
        assert_eq!(op.bytes[0], 0x06);
        assert_eq!(op.bytes[1], 0x42);
    }

    #[test]
    fn prepare_write_applies_payload_transform() {
        let c = cfg(r#"
structs:
  cmd:
    - {name: val, type: uint8}
apis:
  cmd:
    write:
      transport: HID_IO
      chunk_size: 1
      payload_transform: "builtin:double_all"
"#);
        let mut exec = ApiExecutor::new(&c);
        exec.register_builtin("builtin:double_all", |b| b.iter().map(|x| x * 2).collect());
        let mut values = HashMap::new();
        values.insert("val".to_string(), FieldValue::U8(3));
        let op = exec.prepare_write("cmd", &values).unwrap();
        assert_eq!(op.bytes[0], 6, "transform doubled the byte");
    }

    #[test]
    fn prepare_write_unknown_transform_error() {
        let c = cfg(r#"
structs:
  cmd:
    - {name: val, type: uint8}
apis:
  cmd:
    write:
      transport: HID_IO
      chunk_size: 1
      payload_transform: "builtin:missing"
"#);
        let exec = ApiExecutor::new(&c);
        let mut values = HashMap::new();
        values.insert("val".to_string(), FieldValue::U8(1));
        assert!(matches!(
            exec.prepare_write("cmd", &values).unwrap_err(),
            ApiError::UnknownTransform(_)
        ));
    }

    #[test]
    fn prepare_read_builds_outgoing_request() {
        let c = cfg(r#"
structs:
  battery_status:
    outgoing:
      - {name: report_id, type: uint8, constant: 0x06}
      - {name: command,   type: uint8, constant: 0xB7}
    incoming:
      - {name: report_id, type: uint8, constant: 0x06}
      - {name: command,   type: uint8, constant: 0xB7}
      - {name: level,     type: uint8}
apis:
  battery_status:
    read: {transport: HID_IO, chunk_size: 4}
"#);
        let exec = ApiExecutor::new(&c);
        let op = exec.prepare_read("battery_status").unwrap();
        assert_eq!(op.request_bytes.len(), 4);
        assert_eq!(&op.request_bytes[..2], [0x06, 0xB7]);
        assert_eq!(op.transport, Transport::HidIo);
        assert_eq!(op.chunk_size, 4);
    }

    #[test]
    fn parse_response_decodes_incoming() {
        let c = cfg(r#"
structs:
  battery_status:
    outgoing:
      - {name: report_id, type: uint8, constant: 0x06}
      - {name: command,   type: uint8, constant: 0xB7}
    incoming:
      - {name: report_id, type: uint8, constant: 0x06}
      - {name: command,   type: uint8, constant: 0xB7}
      - {name: level,     type: uint8, range: [0, 8]}
apis:
  battery_status:
    read: {transport: HID_IO, chunk_size: 64}
"#);
        let exec = ApiExecutor::new(&c);
        // response bytes: report_id, command, level=5, trailing padding
        let response = [0x06u8, 0xB7, 5, 0, 0];
        let result = exec.parse_response("battery_status", &response).unwrap();
        assert_eq!(result["level"], FieldValue::U8(5));
        assert!(!result.contains_key("report_id"), "constants excluded");
    }

    #[test]
    fn prepare_write_hid_feature_transport() {
        let c = cfg(r#"
structs:
  draw_bitmap:
    - {name: data, type: bytearray, size: 4}
apis:
  draw_bitmap:
    write: {transport: HID_FEATURE, chunk_size: 4}
"#);
        let exec = ApiExecutor::new(&c);
        let mut values = HashMap::new();
        values.insert(
            "data".to_string(),
            FieldValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        );
        let op = exec.prepare_write("draw_bitmap", &values).unwrap();
        assert_eq!(op.transport, Transport::HidFeature);
        assert_eq!(op.bytes, [0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn unknown_api_returns_error() {
        let empty = DeviceConfig::default();
        let exec = ApiExecutor::new(&empty);
        assert!(matches!(
            exec.prepare_write("ghost", &HashMap::new()).unwrap_err(),
            ApiError::UnknownApi(_)
        ));
        assert!(matches!(
            exec.prepare_read("ghost").unwrap_err(),
            ApiError::UnknownApi(_)
        ));
    }

    #[test]
    fn no_write_op_returns_error() {
        let c = cfg(r#"
structs:
  s:
    outgoing: [{name: id, type: uint8, constant: 0x06}]
    incoming: [{name: id, type: uint8, constant: 0x06}, {name: v, type: uint8}]
apis:
  s:
    read: {transport: HID_IO, chunk_size: 4}
"#);
        let exec = ApiExecutor::new(&c);
        assert!(matches!(
            exec.prepare_write("s", &HashMap::new()).unwrap_err(),
            ApiError::NoWriteOp(_)
        ));
    }

    #[test]
    fn no_read_op_returns_error() {
        let c = cfg(r#"
structs:
  save_to_flash:
    - {name: id, type: uint8, constant: 0x06}
    - {name: cmd, type: uint8, constant: 0x09}
apis:
  save_to_flash:
    write: {transport: HID_IO, chunk_size: 4}
"#);
        let exec = ApiExecutor::new(&c);
        assert!(matches!(
            exec.prepare_read("save_to_flash").unwrap_err(),
            ApiError::NoReadOp(_)
        ));
        assert!(matches!(
            exec.parse_response("save_to_flash", &[]).unwrap_err(),
            ApiError::NoReadOp(_)
        ));
    }
}
