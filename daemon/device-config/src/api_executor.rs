// API execution layer: translates named API calls into padded byte payloads
// (and parses responses back).  No I/O lives here — the engine drives the
// actual transport after calling prepare_write / prepare_read / parse_response.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use crate::codec::{Codec, CodecError, FieldValue};
use crate::{ApiDef, ApiOp, DeviceConfig, Transport, WriteApi, WriteStep};

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

/// One physical action to perform, in order, to carry out a write API call.
#[derive(Debug, PartialEq)]
pub enum WriteAction {
    /// Send `payload` over `transport`.
    Send {
        payload: Vec<u8>,
        transport: Transport,
    },
    /// Pause before the next action (e.g. the 1.5s the firmware blocks
    /// commands for after `save_to_flash` on some devices).
    Sleep(Duration),
}

/// Ordered actions ready to execute for a write API call. Most API calls
/// produce a single `Send`; multi-packet APIs (e.g. `draw_bitmap`) or
/// multi-message protocols (e.g. a parametric EQ band write split into a
/// name message, a data message, and a commit message) produce more, with
/// `Sleep` actions interleaved where the device needs a pause between them.
#[derive(Debug)]
pub struct WriteOp {
    pub actions: Vec<WriteAction>,
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

/// A registered payload transform.  Returns one packet for single-chunk APIs,
/// or multiple packets for APIs like `draw_bitmap` that require a split send.
pub type BuiltinFn = Box<dyn Fn(&[u8]) -> Vec<Vec<u8>> + Send + Sync>;

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
        f: impl Fn(&[u8]) -> Vec<Vec<u8>> + Send + Sync + 'static,
    ) {
        self.builtins.insert(name.into(), Box::new(f));
    }

    /// Serialise `values` once, then run every step of the write API's
    /// action sequence against that same serialisation — matching the
    /// vendor spec's convention of several `api-write`/`chunk` calls sharing
    /// one `payload` variable. Each step applies its own `payload_transform`
    /// and pads to its own `chunk_size`; constant fields are filled
    /// automatically.
    pub fn prepare_write(
        &self,
        api_name: &str,
        values: &HashMap<String, FieldValue>,
    ) -> Result<WriteOp, ApiError> {
        let write_api = self.write_api(api_name)?;
        let bytes = self.codec.serialize(api_name, values)?;

        let mut actions = Vec::new();
        match write_api {
            WriteApi::Single(op) => {
                self.push_op_actions(op, &bytes, &mut actions)?;
            }
            WriteApi::Sequence { steps } => {
                for step in steps {
                    match step {
                        WriteStep::Op {
                            transport,
                            chunk_size,
                            payload_transform,
                        } => {
                            let op = ApiOp {
                                transport: transport.clone(),
                                chunk_size: *chunk_size,
                                payload_transform: payload_transform.clone(),
                            };
                            self.push_op_actions(&op, &bytes, &mut actions)?;
                        }
                        WriteStep::Sleep { sleep_ms } => {
                            actions.push(WriteAction::Sleep(Duration::from_millis(*sleep_ms)));
                        }
                    }
                }
            }
        }
        Ok(WriteOp { actions })
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

    fn write_api(&self, api_name: &str) -> Result<&WriteApi, ApiError> {
        self.apis
            .get(api_name)
            .ok_or_else(|| ApiError::UnknownApi(api_name.to_string()))?
            .write
            .as_ref()
            .ok_or_else(|| ApiError::NoWriteOp(api_name.to_string()))
    }

    /// Apply `op`'s payload transform to `bytes` and pad each resulting
    /// payload to `op.chunk_size`, pushing one `WriteAction::Send` per
    /// payload.
    fn push_op_actions(
        &self,
        op: &ApiOp,
        bytes: &[u8],
        actions: &mut Vec<WriteAction>,
    ) -> Result<(), ApiError> {
        let mut payloads = self.apply_transform(bytes.to_vec(), &op.payload_transform)?;
        for p in &mut payloads {
            pad(p, op.chunk_size as usize);
            actions.push(WriteAction::Send {
                payload: std::mem::take(p),
                transport: op.transport.clone(),
            });
        }
        Ok(())
    }

    fn read_op(&self, api_name: &str) -> Result<&ApiOp, ApiError> {
        self.apis
            .get(api_name)
            .ok_or_else(|| ApiError::UnknownApi(api_name.to_string()))?
            .read
            .as_ref()
            .ok_or_else(|| ApiError::NoReadOp(api_name.to_string()))
    }

    fn apply_transform(
        &self,
        bytes: Vec<u8>,
        name: &Option<String>,
    ) -> Result<Vec<Vec<u8>>, ApiError> {
        if let Some(n) = name {
            let f = self
                .builtins
                .get(n.as_str())
                .ok_or_else(|| ApiError::UnknownTransform(n.clone()))?;
            Ok(f(&bytes))
        } else {
            Ok(vec![bytes])
        }
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

    /// Extract the `Send` payloads from a `WriteOp`, in order, ignoring any
    /// `Sleep` actions.
    fn send_payloads(op: &WriteOp) -> Vec<&[u8]> {
        op.actions
            .iter()
            .filter_map(|a| match a {
                WriteAction::Send { payload, .. } => Some(payload.as_slice()),
                WriteAction::Sleep(_) => None,
            })
            .collect()
    }

    fn send_transport(op: &WriteOp, i: usize) -> &Transport {
        match &op.actions[i] {
            WriteAction::Send { transport, .. } => transport,
            WriteAction::Sleep(_) => panic!("action {i} is a Sleep, not a Send"),
        }
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
        let payloads = send_payloads(&op);
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].len(), 8);
        assert_eq!(&payloads[0][..2], [0x06, 0x09]);
        assert_eq!(&payloads[0][2..], [0u8; 6]);
        assert_eq!(*send_transport(&op, 0), Transport::HidIo);
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
        let payloads = send_payloads(&op);
        assert_eq!(payloads[0][0], 0x06);
        assert_eq!(payloads[0][1], 0x42);
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
        exec.register_builtin("builtin:double_all", |b| {
            vec![b.iter().map(|x| x * 2).collect()]
        });
        let mut values = HashMap::new();
        values.insert("val".to_string(), FieldValue::U8(3));
        let op = exec.prepare_write("cmd", &values).unwrap();
        assert_eq!(send_payloads(&op)[0][0], 6, "transform doubled the byte");
    }

    #[test]
    fn prepare_write_sequence_runs_steps_in_order_with_sleep() {
        let c = cfg(r#"
structs:
  save_to_flash:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x09}
apis:
  save_to_flash:
    write:
      steps:
        - {transport: HID_IO, chunk_size: 4}
        - {sleep_ms: 1500}
"#);
        let exec = ApiExecutor::new(&c);
        let op = exec
            .prepare_write("save_to_flash", &HashMap::new())
            .unwrap();
        assert_eq!(op.actions.len(), 2);
        assert_eq!(
            op.actions[0],
            WriteAction::Send {
                payload: vec![0x06, 0x09, 0, 0],
                transport: Transport::HidIo,
            }
        );
        assert_eq!(
            op.actions[1],
            WriteAction::Sleep(Duration::from_millis(1500))
        );
    }

    #[test]
    fn prepare_write_sequence_multi_message_each_step_own_transform() {
        let c = cfg(r#"
structs:
  parametric_eq:
    - {name: report_id, type: uint8, constant: 0x00}
    - {name: name,       type: varstring, size: 4}
apis:
  parametric_eq:
    write:
      steps:
        - transport: HID_IO
          chunk_size: 4
          payload_transform: "builtin:first_byte_only"
        - transport: HID_IO
          chunk_size: 2
          payload_transform: "builtin:last_byte_only"
"#);
        let mut exec = ApiExecutor::new(&c);
        exec.register_builtin("builtin:first_byte_only", |b| vec![vec![b[0]]]);
        exec.register_builtin("builtin:last_byte_only", |b| vec![vec![*b.last().unwrap()]]);
        let mut values = HashMap::new();
        // Exactly `size` bytes: a sized varstring is now zero-padded to
        // `size` on write, so a shorter string's last byte would be padding
        // (0x00), not the string's own last character — see codec.rs.
        values.insert("name".to_string(), FieldValue::Str("EQ12".to_string()));
        let op = exec.prepare_write("parametric_eq", &values).unwrap();
        let payloads = send_payloads(&op);
        assert_eq!(payloads.len(), 2);
        assert_eq!(
            payloads[0],
            [0x00, 0, 0, 0],
            "step 1: first byte, padded to 4"
        );
        assert_eq!(payloads[1], [b'2', 0], "step 2: last byte, padded to 2");
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
        assert_eq!(*send_transport(&op, 0), Transport::HidFeature);
        assert_eq!(send_payloads(&op)[0], [0xDE, 0xAD, 0xBE, 0xEF]);
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
