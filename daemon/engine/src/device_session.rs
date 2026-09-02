// Per-device runtime: wires the device-config DSL components (ApiExecutor,
// SyncReader, SyncDispatcher) to HidTransport, running the init sequence and
// the ongoing sync event read loop.

use std::collections::HashMap;
use std::os::unix::io::OwnedFd;
use std::time::Duration;

use device_config::api_executor::{ApiExecutor, ReadOp, WriteAction};
use device_config::codec::FieldValue;
use device_config::sync_dispatcher::{DispatchResult, EmitEvent, SyncDispatcher};
use device_config::sync_reader::SyncReader;
use device_config::{DeviceConfig, LifecycleCall, SyncReadEntry, Transport};
use hid_transport::{HidTransport, ReadError};
use serde_yaml::Value as Yaml;
use tokio::sync::mpsc;
use tracing::warn;

use crate::engine_error::EngineError;
use crate::state::DeviceCommand;

// ── DeviceSession ─────────────────────────────────────────────────────────────

/// Owns a device's HID transport and its resolved DSL config.  Provides the
/// full device lifecycle: init sequence, startup sync read, and the ongoing
/// async sync event loop.
pub struct DeviceSession {
    config: DeviceConfig,
    transport: HidTransport,
}

impl DeviceSession {
    pub fn new(config: DeviceConfig, fd: OwnedFd) -> std::io::Result<Self> {
        Ok(Self {
            config,
            transport: HidTransport::from_fd(fd)?,
        })
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Run the complete device init sequence:
    /// 1. `lifecycle.init` commands
    /// 2. Startup sync read (populates initial state)
    /// 3. `lifecycle.post_init` commands
    ///
    /// Returns all `EmitEvent`s produced by the sync read and any lifecycle
    /// calls that trigger reads (e.g. `sync_all`).
    pub async fn device_init(&mut self) -> Result<Vec<EmitEvent>, EngineError> {
        let mut events = self.run_lifecycle_hook("init").await?;
        events.extend(self.run_sync_read().await?);
        events.extend(self.run_lifecycle_hook("post_init").await?);
        Ok(events)
    }

    /// Execute all calls in the named lifecycle hook (`init`, `post_init`, or
    /// `shutdown`) and collect any emitted events.
    pub async fn run_lifecycle_hook(&mut self, hook: &str) -> Result<Vec<EmitEvent>, EngineError> {
        let calls = lifecycle_calls(&self.config, hook)?.to_vec();
        let mut events = Vec::new();
        for call in &calls {
            events.extend(self.dispatch_lifecycle_call(call).await?);
        }
        Ok(events)
    }

    /// Iterate all `sync_read` entries, request each struct from the device,
    /// and map the responses to `EmitEvent`s.
    pub async fn run_sync_read(&mut self) -> Result<Vec<EmitEvent>, EngineError> {
        let entries: Vec<SyncReadEntry> = {
            let sr = SyncReader::new(&self.config);
            sr.entries().to_vec()
        };

        let mut events = Vec::new();
        for entry in &entries {
            let read_op = {
                let api = ApiExecutor::new(&self.config);
                api.prepare_read(&entry.struct_name)
                    .map_err(EngineError::Api)?
            };

            let response = self.execute_read_op(&read_op).await?;

            let fields = {
                let api = ApiExecutor::new(&self.config);
                api.parse_response(&entry.struct_name, &response)
                    .map_err(EngineError::Api)?
            };

            let evs = {
                let sr = SyncReader::new(&self.config);
                sr.map_entry(entry, &fields)
                    .map_err(EngineError::SyncRead)?
            };
            events.extend(evs);
        }
        Ok(events)
    }

    /// Read one raw interrupt report from the device, ignoring report content.
    /// Used in the reconnect loop to wait reactively for any sign of life from
    /// the dongle (e.g. a wireless-connection-changed notification) instead of
    /// sleeping a fixed interval between `device_init` retries.
    pub async fn read_any_report(&mut self, timeout: Duration) -> Result<Vec<u8>, EngineError> {
        self.transport
            .read_interrupt(timeout)
            .await
            .map_err(|e| match e {
                ReadError::Io(io_e) => EngineError::Io(io_e),
                ReadError::Timeout => EngineError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "read_any_report timed out",
                )),
            })
    }

    /// Dispatch a raw HID sync report through the sync event table.
    /// Returns `Ok(None)` when the command byte has no entry in `sync_events`.
    #[allow(dead_code)] // used in tests and by the upcoming D-Bus layer (E4)
    pub fn dispatch_sync_report(
        &self,
        report: &[u8],
    ) -> Result<Option<DispatchResult>, EngineError> {
        let dispatcher = SyncDispatcher::new(&self.config);
        dispatcher.dispatch(report).map_err(EngineError::Dispatch)
    }

    #[allow(dead_code)] // public API retained for callers that don't need command support
    /// Read sync reports in a loop and forward `EmitEvent`s to `tx`.
    /// Returns `Ok(())` when `tx` is dropped (engine shutting down) or when
    /// the device sends EOF.  Returns `Err` on unrecoverable transport errors.
    pub async fn run_event_loop(&mut self, tx: mpsc::Sender<EmitEvent>) -> Result<(), EngineError> {
        loop {
            let report = match self
                .transport
                .read_interrupt(Duration::from_millis(5000))
                .await
            {
                Ok(r) if r.is_empty() => return Ok(()), // EOF — device disconnected
                Ok(r) => r,
                Err(ReadError::Timeout) => continue,
                Err(ReadError::Io(e)) => return Err(EngineError::Io(e)),
            };

            let result = {
                let dispatcher = SyncDispatcher::new(&self.config);
                match dispatcher.dispatch(&report) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("malformed sync report: {e}");
                        continue;
                    }
                }
            };

            if let Some(dr) = result {
                if let Some(emit) = dr.emit {
                    if tx.send(emit).await.is_err() {
                        return Ok(()); // receiver dropped
                    }
                }
                for effect in dr.side_effects {
                    if let Err(e) = self.dispatch_call_by_name(&effect.call, None).await {
                        warn!("side effect '{}' failed: {e}", effect.call);
                    }
                }
            }
        }
    }

    /// Like `run_event_loop` but also handles `DeviceCommand`s from the D-Bus
    /// layer.  Returns `Ok(())` on clean shutdown or `Err` on transport failure.
    pub async fn run_event_loop_with_commands(
        &mut self,
        tx: mpsc::Sender<EmitEvent>,
        mut cmd_rx: mpsc::Receiver<DeviceCommand>,
    ) -> Result<(), EngineError> {
        use tokio::time::MissedTickBehavior;
        let mut periodic = tokio::time::interval(Duration::from_secs(10));
        periodic.set_missed_tick_behavior(MissedTickBehavior::Skip);
        periodic.tick().await; // skip first immediate tick — device_init already ran sync_all

        loop {
            tokio::select! {
                read_result = self.transport.read_interrupt(Duration::from_millis(5000)) => {
                    let report = match read_result {
                        Ok(r) if r.is_empty() => return Ok(()), // EOF
                        Ok(r) => r,
                        Err(ReadError::Timeout) => continue,
                        Err(ReadError::Io(e)) => return Err(EngineError::Io(e)),
                    };

                    let result = {
                        let dispatcher = SyncDispatcher::new(&self.config);
                        match dispatcher.dispatch(&report) {
                            Ok(r) => r,
                            Err(e) => {
                                warn!("malformed sync report: {e}");
                                continue;
                            }
                        }
                    };

                    if let Some(dr) = result {
                        if let Some(emit) = dr.emit {
                            if tx.send(emit).await.is_err() {
                                return Ok(()); // receiver dropped
                            }
                        }
                        for effect in dr.side_effects {
                            match self.dispatch_call_by_name(&effect.call, None).await {
                                Ok(side_events) => {
                                    for ev in side_events {
                                        if tx.send(ev).await.is_err() {
                                            return Ok(());
                                        }
                                    }
                                }
                                Err(e) => warn!("side effect '{}' failed: {e}", effect.call),
                            }
                        }
                    }
                }
                cmd_opt = cmd_rx.recv() => {
                    match cmd_opt {
                        Some(DeviceCommand::WriteApi { api_name, values }) => {
                            if let Err(e) = self.send_api_write(&api_name, &values).await {
                                warn!("D-Bus command '{api_name}' failed: {e}");
                            }
                        }
                        None => return Ok(()), // all senders dropped
                    }
                }
                _ = periodic.tick() => {
                    match self.run_sync_read().await {
                        Ok(events) => {
                            for ev in events {
                                if tx.send(ev).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                        Err(e) => warn!("periodic sync_read failed: {e}"),
                    }
                }
            }
        }
    }

    // ── Private ───────────────────────────────────────────────────────────────

    async fn dispatch_lifecycle_call(
        &mut self,
        call: &LifecycleCall,
    ) -> Result<Vec<EmitEvent>, EngineError> {
        self.dispatch_call_by_name(&call.call, call.args.as_ref())
            .await
    }

    async fn dispatch_call_by_name(
        &mut self,
        name: &str,
        args: Option<&Yaml>,
    ) -> Result<Vec<EmitEvent>, EngineError> {
        match name {
            "enable_sonar" => {
                self.send_api_write("set_sonar_present", &u8_fields(&[("is_present", 1)]))
                    .await?;
                Ok(vec![])
            }
            "disable_sonar" => {
                self.send_api_write("set_sonar_present", &u8_fields(&[("is_present", 0)]))
                    .await?;
                Ok(vec![])
            }
            "reset_eq_preset" => {
                self.send_api_write("selected_eq_preset", &u8_fields(&[("eq_preset", 0)]))
                    .await?;
                Ok(vec![])
            }
            "enable_chatmix" => {
                self.send_api_write("software_chatmix_status", &u8_fields(&[("status", 1)]))
                    .await?;
                Ok(vec![])
            }
            "disable_chatmix" => {
                self.send_api_write("software_chatmix_status", &u8_fields(&[("status", 0)]))
                    .await?;
                Ok(vec![])
            }
            "save_to_flash" => {
                self.send_api_write("save_to_flash", &HashMap::new())
                    .await?;
                Ok(vec![])
            }
            "discord_certified_set_attributes" => {
                let fields = yaml_to_field_map(args);
                self.send_api_write("discord_certified_attributes", &fields)
                    .await?;
                Ok(vec![])
            }
            "sync_all" | "send_init_wireless_connection_battery_status" => {
                self.run_sync_read().await
            }
            unknown => Err(EngineError::UnknownLifecycleCall(unknown.to_string())),
        }
    }

    #[cfg(test)]
    pub async fn write_api_direct(
        &mut self,
        api_name: &str,
        values: HashMap<String, FieldValue>,
    ) -> Result<(), EngineError> {
        self.send_api_write(api_name, &values).await
    }

    async fn send_api_write(
        &mut self,
        api_name: &str,
        values: &HashMap<String, FieldValue>,
    ) -> Result<(), EngineError> {
        let op = {
            let api = make_api_executor(&self.config);
            api.prepare_write(api_name, values)
                .map_err(EngineError::Api)?
        };
        for action in &op.actions {
            match action {
                WriteAction::Send { payload, transport } => match transport {
                    Transport::HidIo => {
                        self.transport
                            .write_interrupt(payload)
                            .await
                            .map_err(EngineError::Io)?;
                    }
                    Transport::HidFeature => {
                        self.transport
                            .write_feature(payload)
                            .map_err(EngineError::Io)?;
                    }
                },
                WriteAction::Sleep(duration) => {
                    tokio::time::sleep(*duration).await;
                }
            }
        }
        Ok(())
    }

    async fn execute_read_op(&mut self, op: &ReadOp) -> Result<Vec<u8>, EngineError> {
        match op.transport {
            Transport::HidIo => {
                self.transport
                    .write_interrupt(&op.request_bytes)
                    .await
                    .map_err(EngineError::Io)?;

                // The interrupt stream carries both command responses and
                // unsolicited device notifications.  Drain any notification
                // reports whose first byte (report_id) does not match the
                // expected response ID before returning the real response.
                let expected_id = op.request_bytes.first().copied();
                for _ in 0..8u8 {
                    let response = self
                        .transport
                        .read_interrupt(Duration::from_millis(1000))
                        .await
                        .map_err(|e| match e {
                            ReadError::Io(io_e) => EngineError::Io(io_e),
                            ReadError::Timeout => EngineError::Io(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "sync read timed out",
                            )),
                        })?;
                    if expected_id.is_none_or(|id| response.first() == Some(&id)) {
                        return Ok(response);
                    }
                    // Wrong report ID — async notification buffered before response.
                }
                Err(EngineError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "sync read: too many unexpected reports before response",
                )))
            }
            Transport::HidFeature => {
                let mut buf = vec![0u8; op.chunk_size];
                if let Some(&report_id) = op.request_bytes.first() {
                    buf[0] = report_id;
                }
                let n = self
                    .transport
                    .read_feature(&mut buf)
                    .map_err(EngineError::Io)?;
                buf.truncate(n);
                Ok(buf)
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_api_executor(config: &DeviceConfig) -> ApiExecutor<'_> {
    use device_config::builtins::{
        dim_timer_write_payload, graphic_eq_gains_payload, high_gain_write_payload,
        muted_mic_brightness_write_payload, named_slot_signed_gains_payload_args,
        nova_pro_omni_mic_noise_reduction_write_payload, parametric_eq_bands_payload_args,
        parametric_eq_commit_payload_args, parametric_eq_name_payload_args,
        parametric_eq_named_slot_payload_args, power_timer_write_payload,
    };
    let mut exec = ApiExecutor::new(config);
    // Non-EQ builtins: genuinely device-specific logic, not data-parameterizable
    // the same way — they ignore `payload_transform_args`.
    exec.register_builtin("builtin:high_gain_write", |b, _args| {
        high_gain_write_payload(b)
    });
    exec.register_builtin("builtin:dim_timer_write", |b, _args| {
        dim_timer_write_payload(b)
    });
    exec.register_builtin("builtin:power_timer_write", |b, _args| {
        power_timer_write_payload(b)
    });
    exec.register_builtin("builtin:muted_mic_brightness_write", |b, _args| {
        muted_mic_brightness_write_payload(b)
    });
    exec.register_builtin(
        "builtin:nova_pro_omni_mic_noise_reduction_write",
        |b, _args| nova_pro_omni_mic_noise_reduction_write_payload(b),
    );

    // Generic EQ builtins: one function per shape, parameterised entirely by
    // `payload_transform_args` in each device's own YAML — see builtins.rs.
    exec.register_builtin("builtin:graphic_eq_gains", graphic_eq_gains_payload);
    exec.register_builtin(
        "builtin:parametric_eq_name",
        parametric_eq_name_payload_args,
    );
    exec.register_builtin(
        "builtin:parametric_eq_bands",
        parametric_eq_bands_payload_args,
    );
    exec.register_builtin(
        "builtin:parametric_eq_commit",
        parametric_eq_commit_payload_args,
    );
    exec.register_builtin(
        "builtin:parametric_eq_named_slot",
        parametric_eq_named_slot_payload_args,
    );
    exec.register_builtin(
        "builtin:named_slot_signed_gains",
        named_slot_signed_gains_payload_args,
    );
    exec
}

fn lifecycle_calls<'a>(
    config: &'a DeviceConfig,
    hook: &str,
) -> Result<&'a [LifecycleCall], EngineError> {
    let lc = match config.lifecycle.as_ref() {
        Some(lc) => lc,
        None => {
            return match hook {
                "init" | "post_init" | "shutdown" => Ok(&[]),
                _ => Err(EngineError::UnknownLifecycleHook(hook.to_string())),
            };
        }
    };
    match hook {
        "init" => Ok(lc.init.as_deref().unwrap_or(&[])),
        "post_init" => Ok(lc.post_init.as_deref().unwrap_or(&[])),
        "shutdown" => Ok(lc.shutdown.as_deref().unwrap_or(&[])),
        _ => Err(EngineError::UnknownLifecycleHook(hook.to_string())),
    }
}

fn u8_fields(pairs: &[(&str, u8)]) -> HashMap<String, FieldValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), FieldValue::U8(*v)))
        .collect()
}

fn yaml_to_field_map(yaml: Option<&Yaml>) -> HashMap<String, FieldValue> {
    let mut map = HashMap::new();
    if let Some(Yaml::Mapping(m)) = yaml {
        for (k, v) in m {
            if let Yaml::String(key) = k {
                if let Some(fv) = yaml_to_field_value(v) {
                    map.insert(key.clone(), fv);
                }
            }
        }
    }
    map
}

fn yaml_to_field_value(val: &Yaml) -> Option<FieldValue> {
    match val {
        Yaml::Bool(b) => Some(FieldValue::U8(if *b { 1 } else { 0 })),
        Yaml::Number(n) => {
            if let Some(u) = n.as_u64() {
                Some(if u <= 0xFF {
                    FieldValue::U8(u as u8)
                } else if u <= 0xFFFF {
                    FieldValue::U16(u as u16)
                } else {
                    FieldValue::U32(u as u32)
                })
            } else {
                n.as_f64().map(|f| FieldValue::F32(f as f32))
            }
        }
        _ => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};

    fn make_pair() -> (OwnedFd, OwnedFd) {
        // SOCK_SEQPACKET preserves message boundaries: each write() is a
        // separate read(), matching the per-report semantics of real hidraw fds.
        socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::SOCK_CLOEXEC,
        )
        .expect("socketpair")
    }

    fn cfg(yaml: &str) -> DeviceConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    // ── E1-S4: init sequence ──────────────────────────────────────────────────

    #[tokio::test]
    async fn device_init_with_empty_config_succeeds() {
        let (engine_fd, _peer) = make_pair();
        let mut session = DeviceSession::new(cfg("{}"), engine_fd).expect("from_fd");
        let events = session.device_init().await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn device_init_unknown_call_returns_error() {
        let (engine_fd, _peer) = make_pair();
        let mut session = DeviceSession::new(
            cfg(r#"
lifecycle:
  init:
    - call: reticulate_splines
"#),
            engine_fd,
        )
        .expect("from_fd");
        assert!(matches!(
            session.device_init().await.unwrap_err(),
            EngineError::UnknownLifecycleCall(s) if s == "reticulate_splines"
        ));
    }

    #[tokio::test]
    async fn device_init_sends_write_command() {
        let (engine_fd, peer_fd) = make_pair();
        let session = DeviceSession::new(
            cfg(r#"
structs:
  save_to_flash:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x09}
apis:
  save_to_flash:
    write: {transport: HID_IO, chunk_size: 8}
lifecycle:
  init:
    - call: save_to_flash
"#),
            engine_fd,
        )
        .expect("from_fd");

        let task = tokio::spawn(async move {
            let mut s = session;
            s.device_init().await
        });

        // Read what the engine sent.
        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("engine should have sent a report");

        let events = task.await.unwrap().unwrap();
        assert!(events.is_empty());
        // First two bytes are the struct constants; rest is zero padding.
        assert_eq!(received[0], 0x06); // report_id constant
        assert_eq!(received[1], 0x09); // command constant
        assert_eq!(&received[2..], &[0u8; 6]);
    }

    #[tokio::test]
    async fn reset_eq_preset_lifecycle_selects_flat() {
        let (engine_fd, peer_fd) = make_pair();
        let session = DeviceSession::new(
            cfg(r#"
structs:
  selected_eq_preset:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x2E}
    - {name: eq_preset, type: uint8, range: [0, 18]}
apis:
  selected_eq_preset:
    write: {transport: HID_IO, chunk_size: 8}
lifecycle:
  init:
    - call: reset_eq_preset
"#),
            engine_fd,
        )
        .expect("from_fd");

        let task = tokio::spawn(async move {
            let mut s = session;
            s.device_init().await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("engine should have sent the flat preset report");

        task.await.unwrap().unwrap();
        assert_eq!(&received[..3], &[0x06, 0x2E, 0x00]);
    }

    #[tokio::test]
    async fn shutdown_hook_sends_correct_bytes() {
        let (engine_fd, peer_fd) = make_pair();
        let session = DeviceSession::new(
            cfg(r#"
structs:
  save_to_flash:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x09}
apis:
  save_to_flash:
    write: {transport: HID_IO, chunk_size: 4}
lifecycle:
  shutdown:
    - call: save_to_flash
"#),
            engine_fd,
        )
        .expect("from_fd");

        let task = tokio::spawn(async move {
            let mut s = session;
            s.run_lifecycle_hook("shutdown").await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();

        task.await.unwrap().unwrap();
        assert_eq!(&received[..2], &[0x06, 0x09]);
    }

    #[tokio::test]
    async fn run_lifecycle_hook_unknown_name_errors() {
        let (engine_fd, _peer) = make_pair();
        let mut session = DeviceSession::new(cfg("{}"), engine_fd).expect("from_fd");
        assert!(matches!(
            session.run_lifecycle_hook("flurp").await.unwrap_err(),
            EngineError::UnknownLifecycleHook(s) if s == "flurp"
        ));
    }

    #[tokio::test]
    async fn run_sync_read_sends_request_and_maps_response() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  audio_settings:
    outgoing:
      - {name: report_id,  type: uint8, constant: 0x00}
      - {name: command,    type: uint8, constant: 0x45}
    incoming:
      - {name: report_id,  type: uint8, constant: 0x00}
      - {name: command,    type: uint8, constant: 0x45}
      - {name: mic_volume, type: uint8}
apis:
  audio_settings:
    read: {transport: HID_IO, chunk_size: 8}
sync_read:
  - struct: audio_settings
    maps:
      - {emit: mic_volume_changed, field: mic_volume}
"#);
        let session_config = config.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();

        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(session_config, engine_fd).expect("from_fd");
            let result = s.run_sync_read().await;
            let _ = tx.send(result);
        });

        // Serve the read: receive the request, send back a response.
        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        peer.read_interrupt(Duration::from_millis(500))
            .await
            .expect("engine should send read request");

        let mut resp = vec![0u8; 8];
        resp[0] = 0x00; // report_id
        resp[1] = 0x45; // command
        resp[2] = 42; // mic_volume
        peer.write_interrupt(&resp).await.unwrap();

        task.await.unwrap();
        let events = rx.await.unwrap().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].signal, "mic_volume_changed");
    }

    // ── E1-S5: async event loop ───────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_sync_report_emits_known_event() {
        let (engine_fd, _peer) = make_pair();
        let session = DeviceSession::new(
            cfg(r#"
sync_events:
  0x42:
    emit: battery_changed
    fields:
      - {name: level, byte: 2}
"#),
            engine_fd,
        )
        .expect("from_fd");

        let report = [0x00u8, 0x42, 80, 0, 0, 0, 0, 0]; // level = 80
        let result = session
            .dispatch_sync_report(&report)
            .unwrap()
            .expect("should dispatch");

        let emit = result.emit.unwrap();
        assert_eq!(emit.signal, "battery_changed");
        assert_eq!(
            emit.fields["level"],
            device_config::sync_dispatcher::EventValue::Field(FieldValue::U8(80))
        );
    }

    #[tokio::test]
    async fn dispatch_sync_report_returns_none_for_unknown_command() {
        let (engine_fd, _peer) = make_pair();
        let session = DeviceSession::new(cfg("{}"), engine_fd).expect("from_fd");
        let report = [0x00u8, 0xFF, 0x00, 0x00];
        assert!(session.dispatch_sync_report(&report).unwrap().is_none());
    }

    #[tokio::test]
    async fn run_event_loop_dispatches_report_to_channel() {
        let (engine_fd, peer_fd) = make_pair();
        let session = DeviceSession::new(
            cfg(r#"
sync_events:
  0x10:
    emit: status_update
    fields: []
"#),
            engine_fd,
        )
        .expect("from_fd");

        let (event_tx, mut event_rx) = mpsc::channel(8);

        let task = tokio::spawn(async move {
            let mut s = session;
            s.run_event_loop(event_tx).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let report = [0x00u8, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        peer.write_interrupt(&report).await.unwrap();

        let event = tokio::time::timeout(Duration::from_millis(500), event_rx.recv())
            .await
            .expect("timed out waiting for event")
            .expect("channel closed prematurely");
        assert_eq!(event.signal, "status_update");

        // Shut down: drop the channel so the loop exits on next recv.
        drop(event_rx);
        // Also close the peer to trigger EOF / IO error so the loop unblocks.
        drop(peer);
        // The task should finish (either Ok or Err(Io)).
        let _ = tokio::time::timeout(Duration::from_millis(500), task).await;
    }

    #[tokio::test]
    async fn run_event_loop_skips_unknown_command_bytes() {
        let (engine_fd, peer_fd) = make_pair();
        let session = DeviceSession::new(
            cfg(r#"
sync_events:
  0x20:
    emit: known_event
    fields: []
"#),
            engine_fd,
        )
        .expect("from_fd");

        let (event_tx, mut event_rx) = mpsc::channel(8);

        let task = tokio::spawn(async move {
            let mut s = session;
            s.run_event_loop(event_tx).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        // Unknown command — should be skipped without error.
        peer.write_interrupt(&[0x00u8, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
            .await
            .unwrap();
        // Known command — should be dispatched.
        peer.write_interrupt(&[0x00u8, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_millis(500), event_rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_eq!(event.signal, "known_event");

        drop(event_rx);
        drop(peer);
        let _ = tokio::time::timeout(Duration::from_millis(500), task).await;
    }

    // ── E6-S3: custom EQ write (builtin:custom_eq_gains) ─────────────────────

    #[tokio::test]
    async fn write_custom_eq_converts_float_gains_to_firmware_bytes() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  custom_eq:
    - {name: report_id, type: uint8,   constant: 0x06}
    - {name: command,   type: uint8,   constant: 0x33}
    - {name: gain1,     type: float32, range: [-10.0, 10.0]}
    - {name: gain2,     type: float32, range: [-10.0, 10.0]}
    - {name: gain3,     type: float32, range: [-10.0, 10.0]}
    - {name: gain4,     type: float32, range: [-10.0, 10.0]}
    - {name: gain5,     type: float32, range: [-10.0, 10.0]}
    - {name: gain6,     type: float32, range: [-10.0, 10.0]}
    - {name: gain7,     type: float32, range: [-10.0, 10.0]}
    - {name: gain8,     type: float32, range: [-10.0, 10.0]}
    - {name: gain9,     type: float32, range: [-10.0, 10.0]}
    - {name: gain10,    type: float32, range: [-10.0, 10.0]}
apis:
  custom_eq:
    write:
      transport: HID_IO
      chunk_size: 64
      payload_transform: "builtin:graphic_eq_gains"
      payload_transform_args: {offset_db: 10.0, clamp_max: 255}
"#);
        let mut values = HashMap::new();
        for i in 1..=10u8 {
            values.insert(format!("gain{i}"), FieldValue::F32(0.0));
        }
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("custom_eq", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        // bytes[0]=0x06 (report_id), bytes[1]=0x33 (command), bytes[2..12]=20 each
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("read failed");
        task.await.unwrap().unwrap();

        assert_eq!(received[0], 0x06);
        assert_eq!(received[1], 0x33);
        assert_eq!(
            &received[2..12],
            &[20u8; 10],
            "all 0 dB gains → firmware 20"
        );
    }

    /// Regression test for a real bug found while building Nova 7 Gen2's EQ
    /// builtins: `codec::write_fv` serialises float32 fields big-endian, but
    /// `gains_to_firmware_values`/`gains_to_firmware_values_7plus` used to
    /// decode them little-endian — silently corrupting every non-zero gain
    /// sent through the real `prepare_write` pipeline (0.0 dB is
    /// endianness-blind, which is why the test above never caught it). Uses
    /// non-zero, asymmetric gains specifically so a BE/LE mismatch fails loudly.
    #[tokio::test]
    async fn write_custom_eq_nonzero_gains_survive_the_full_pipeline() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  custom_eq:
    - {name: report_id, type: uint8,   constant: 0x06}
    - {name: command,   type: uint8,   constant: 0x33}
    - {name: gain1,     type: float32, range: [-10.0, 10.0]}
    - {name: gain2,     type: float32, range: [-10.0, 10.0]}
    - {name: gain3,     type: float32, range: [-10.0, 10.0]}
    - {name: gain4,     type: float32, range: [-10.0, 10.0]}
    - {name: gain5,     type: float32, range: [-10.0, 10.0]}
    - {name: gain6,     type: float32, range: [-10.0, 10.0]}
    - {name: gain7,     type: float32, range: [-10.0, 10.0]}
    - {name: gain8,     type: float32, range: [-10.0, 10.0]}
    - {name: gain9,     type: float32, range: [-10.0, 10.0]}
    - {name: gain10,    type: float32, range: [-10.0, 10.0]}
apis:
  custom_eq:
    write:
      transport: HID_IO
      chunk_size: 64
      payload_transform: "builtin:graphic_eq_gains"
      payload_transform_args: {offset_db: 10.0, clamp_max: 255}
"#);
        let mut values = HashMap::new();
        // -10 dB -> 0, +10 dB -> 40, -5 dB -> 10, +5 dB -> 30, rest flat (20).
        let gains_db = [-10.0, 10.0, -5.0, 5.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        for (i, db) in gains_db.iter().enumerate() {
            values.insert(format!("gain{}", i + 1), FieldValue::F32(*db));
        }
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("custom_eq", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("read failed");
        task.await.unwrap().unwrap();

        assert_eq!(
            &received[2..12],
            &[0, 40, 10, 30, 20, 20, 20, 20, 20, 20],
            "non-zero gains must round-trip through the real codec (big-endian) \
             encoding, not the little-endian shape the builtin used to assume"
        );
    }

    // ── Nova 7 Gen2: parametric EQ multi-step write ───────────────────────────

    /// End-to-end proof of the new `write.steps` sequence primitive: one
    /// logical `SetSettings` call for `parametric_eq` must reach the device
    /// as three separate physical HID reports, in order, each built from the
    /// same serialised struct via its own `payload_transform` builtin.
    #[tokio::test]
    async fn write_parametric_eq_sends_three_messages_in_order() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  parametric_eq:
    - {name: report_id,        type: uint8, constant: 0x00}
    - {name: eq_name_command,  type: uint8, constant: 0xA7}
    - {name: connection_type,  type: uint8, constant: 0x00}
    - {name: preset_type,      type: uint8, range: [0, 1]}
    - {name: eqband_command,   type: uint8, constant: 0x33}
    - {name: update_complete,  type: uint8, constant: 0x27}
    - {name: band1_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band1_filter_type, type: uint8,   range: [0, 6]}
    - {name: band1_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band1_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band2_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band2_filter_type, type: uint8,   range: [0, 6]}
    - {name: band2_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band2_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band3_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band3_filter_type, type: uint8,   range: [0, 6]}
    - {name: band3_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band3_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band4_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band4_filter_type, type: uint8,   range: [0, 6]}
    - {name: band4_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band4_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band5_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band5_filter_type, type: uint8,   range: [0, 6]}
    - {name: band5_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band5_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band6_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band6_filter_type, type: uint8,   range: [0, 6]}
    - {name: band6_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band6_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band7_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band7_filter_type, type: uint8,   range: [0, 6]}
    - {name: band7_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band7_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band8_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band8_filter_type, type: uint8,   range: [0, 6]}
    - {name: band8_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band8_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band9_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band9_filter_type, type: uint8,   range: [0, 6]}
    - {name: band9_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band9_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band10_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band10_filter_type, type: uint8,   range: [0, 6]}
    - {name: band10_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band10_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: name, type: varstring, size: 60}
apis:
  parametric_eq:
    write:
      steps:
        - {transport: HID_IO, chunk_size: 65, payload_transform: "builtin:parametric_eq_name", payload_transform_args: {header_len: 6, band_count: 10}}
        - {transport: HID_IO, chunk_size: 65, payload_transform: "builtin:parametric_eq_bands", payload_transform_args: {header_len: 6, band_count: 10, gain_flavour: signed_tenths_db}}
        - {transport: HID_IO, chunk_size: 65, payload_transform: "builtin:parametric_eq_commit", payload_transform_args: {commit_byte_offset: 5}}
"#);
        let mut values = HashMap::new();
        values.insert("preset_type".to_string(), FieldValue::U8(1));
        values.insert(
            "name".to_string(),
            FieldValue::Str("Bass Boost".to_string()),
        );
        for band in 1..=10u8 {
            let (freq, filter_type, gain, q) = if band == 1 {
                (1000u16, 1u8, -1.2f32, 1.414f32)
            } else {
                (0u16, 0u8, 0.0f32, 0.0f32)
            };
            values.insert(format!("band{band}_frequency"), FieldValue::U16(freq));
            values.insert(
                format!("band{band}_filter_type"),
                FieldValue::U8(filter_type),
            );
            values.insert(format!("band{band}_gain"), FieldValue::F32(gain));
            values.insert(format!("band{band}_q_factor"), FieldValue::F32(q));
        }

        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("parametric_eq", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let msg1 = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("message 1 (name)");
        let msg2 = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("message 2 (bands)");
        let msg3 = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("message 3 (commit)");
        task.await.unwrap().unwrap();

        // Message 1: report_id, 0xA7, connection_type, preset_type, name bytes.
        assert_eq!(&msg1[0..4], [0x00, 0xA7, 0x00, 0x01]);
        assert_eq!(&msg1[4..14], b"Bass Boost");

        // Message 2: report_id, 0x33, connection_type, then band1's re-encoded
        // data (freq 1000 = 0x03E8 BE, filter 1, gain -1.2dB -> -12 (0xF4),
        // Q 1.414 -> 1414 little-endian [0x86, 0x05]).
        assert_eq!(&msg2[0..3], [0x00, 0x33, 0x00]);
        assert_eq!(&msg2[3..9], [0x03, 0xE8, 0x01, 0xF4, 0x86, 0x05]);

        // Message 3: report_id, 0x27 (commit).
        assert_eq!(&msg3[0..2], [0x00, 0x27]);
    }

    // ── E7-S8: Nova 5 parametric EQ multi-step write ──────────────────────────

    /// Nova 5's own parametric EQ write, using the *generic*
    /// `builtin:parametric_eq_name`/`builtin:parametric_eq_bands` (with
    /// Nova 5's own `payload_transform_args`) — proves the
    /// [`parametric_eq_name_payload`]/[`parametric_eq_bands_payload`] core
    /// shared with Nova 7 Gen2 handles a real second device shape correctly:
    /// only 2 messages (no commit step — Nova 5's struct has no
    /// `update_complete` field), a different name-command byte (0xA5 vs
    /// 0xA7), and the unsigned half-dB-offset gain encoding instead of Gen2's
    /// signed-tenths-dB one. Also exercises the real `{sleep_ms: 600}` step
    /// between messages 1 and 2, matching the vendor spec's explicit
    /// inter-message timing requirement for this device.
    #[tokio::test]
    async fn write_nova5_parametric_eq_sends_two_messages_with_sleep_between() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  parametric_eq:
    - {name: report_id,        type: uint8, constant: 0x00}
    - {name: eq_name_command,  type: uint8, constant: 0xA5}
    - {name: connection_type,  type: uint8, constant: 0x00}
    - {name: preset_type,      type: uint8, range: [0, 1]}
    - {name: eqband_command,   type: uint8, constant: 0x33}
    - {name: band1_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band1_filter_type, type: uint8,   range: [1, 5]}
    - {name: band1_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band1_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band2_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band2_filter_type, type: uint8,   range: [1, 5]}
    - {name: band2_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band2_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band3_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band3_filter_type, type: uint8,   range: [1, 5]}
    - {name: band3_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band3_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band4_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band4_filter_type, type: uint8,   range: [1, 5]}
    - {name: band4_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band4_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band5_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band5_filter_type, type: uint8,   range: [1, 5]}
    - {name: band5_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band5_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band6_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band6_filter_type, type: uint8,   range: [1, 5]}
    - {name: band6_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band6_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band7_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band7_filter_type, type: uint8,   range: [1, 5]}
    - {name: band7_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band7_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band8_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band8_filter_type, type: uint8,   range: [1, 5]}
    - {name: band8_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band8_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band9_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band9_filter_type, type: uint8,   range: [1, 5]}
    - {name: band9_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band9_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: band10_frequency,   type: uint16,  range: [20, 20001]}
    - {name: band10_filter_type, type: uint8,   range: [1, 5]}
    - {name: band10_gain,        type: float32, range: [-12.0, 12.0]}
    - {name: band10_q_factor,    type: float32, range: [0.2, 10.0]}
    - {name: name, type: varstring, size: 60}
apis:
  parametric_eq:
    write:
      steps:
        - {transport: HID_IO, chunk_size: 65, payload_transform: "builtin:parametric_eq_name", payload_transform_args: {header_len: 5, band_count: 10}}
        - {sleep_ms: 600}
        - {transport: HID_IO, chunk_size: 65, payload_transform: "builtin:parametric_eq_bands", payload_transform_args: {header_len: 5, band_count: 10, gain_flavour: unsigned_half_db_offset, offset_db: 10.0, clamp_max: 40}}
"#);
        let mut values = HashMap::new();
        values.insert("preset_type".to_string(), FieldValue::U8(1));
        values.insert("name".to_string(), FieldValue::Str("Bass".to_string()));
        for band in 1..=10u8 {
            let (freq, filter_type, gain, q) = if band == 1 {
                (1000u16, 1u8, -1.2f32, 1.414f32)
            } else {
                (0u16, 0u8, 0.0f32, 0.0f32)
            };
            values.insert(format!("band{band}_frequency"), FieldValue::U16(freq));
            values.insert(
                format!("band{band}_filter_type"),
                FieldValue::U8(filter_type),
            );
            values.insert(format!("band{band}_gain"), FieldValue::F32(gain));
            values.insert(format!("band{band}_q_factor"), FieldValue::F32(q));
        }

        let started = std::time::Instant::now();
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("parametric_eq", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let msg1 = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .expect("message 1 (name)");
        let msg2 = peer
            .read_interrupt(Duration::from_secs(2))
            .await
            .expect("message 2 (bands)");
        task.await.unwrap().unwrap();

        // Message 1: report_id, 0xA5 (Nova 5's name command, not Gen2's
        // 0xA7), connection_type, preset_type, name bytes.
        assert_eq!(&msg1[0..4], [0x00, 0xA5, 0x00, 0x01]);
        assert_eq!(&msg1[4..8], b"Bass");

        // Message 2: report_id, 0x33, connection_type, then band1's
        // re-encoded data (freq 1000 = 0x03E8 BE, filter 1, gain -1.2dB ->
        // round(2*(10-1.2))=18 (0x12, unsigned — not Gen2's signed -12), Q
        // 1.414 -> 1414 little-endian [0x86, 0x05]).
        assert_eq!(&msg2[0..3], [0x00, 0x33, 0x00]);
        assert_eq!(&msg2[3..9], [0x03, 0xE8, 0x01, 0x12, 0x86, 0x05]);

        assert!(
            started.elapsed() >= Duration::from_millis(600),
            "the {{sleep_ms: 600}} step between messages must actually elapse"
        );
    }

    // ── E6-S4: EQ preset selection ────────────────────────────────────────────

    #[tokio::test]
    async fn write_selected_eq_preset_sends_correct_bytes() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  selected_eq_preset:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x2E}
    - {name: eq_preset, type: uint8, range: [0, 18]}
apis:
  selected_eq_preset:
    write: {transport: HID_IO, chunk_size: 8}
"#);
        let mut values = HashMap::new();
        values.insert("eq_preset".to_string(), FieldValue::U8(4));
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("selected_eq_preset", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(received[0], 0x06);
        assert_eq!(received[1], 0x2E);
        assert_eq!(received[2], 4);
    }

    // ── E6-S5: line out mode and stream mix ───────────────────────────────────

    #[tokio::test]
    async fn write_line_out_mode_sends_correct_bytes() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  line_out_mode:
    - {name: report_id,     type: uint8, constant: 0x06}
    - {name: command,       type: uint8, constant: 0x43}
    - {name: line_out_mode, type: uint8, range: [1, 2]}
apis:
  line_out_mode:
    write: {transport: HID_IO, chunk_size: 8}
"#);
        let mut values = HashMap::new();
        values.insert("line_out_mode".to_string(), FieldValue::U8(2));
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("line_out_mode", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(&received[..3], &[0x06, 0x43, 2]);
    }

    #[tokio::test]
    async fn write_stream_mix_inserts_unused_byte_and_correct_values() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  stream_mix:
    - {name: report_id,   type: uint8, constant: 0x06}
    - {name: command,     type: uint8, constant: 0x47}
    - {name: stream_main, type: uint8, range: [0, 100]}
    - {name: unused,      type: uint8, constant: 0x00}
    - {name: stream_aux,  type: uint8, range: [0, 100]}
    - {name: stream_mic,  type: uint8, range: [0, 100]}
apis:
  stream_mix:
    write: {transport: HID_IO, chunk_size: 8}
"#);
        let mut values = HashMap::new();
        values.insert("stream_main".to_string(), FieldValue::U8(70));
        values.insert("stream_aux".to_string(), FieldValue::U8(30));
        values.insert("stream_mic".to_string(), FieldValue::U8(50));
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("stream_mix", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(received[0], 0x06);
        assert_eq!(received[1], 0x47);
        assert_eq!(received[2], 70, "stream_main");
        assert_eq!(received[3], 0x00, "unused byte zero");
        assert_eq!(received[4], 30, "stream_aux");
        assert_eq!(received[5], 50, "stream_mic");
    }

    // ── E6-S6: OLED / dim timer ───────────────────────────────────────────────

    #[tokio::test]
    async fn write_dim_timer_maps_minutes_to_firmware_enum() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  dim_timer:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x83}
    - {name: dim_timer, type: uint8, range: [0, 60]}
apis:
  dim_timer:
    write:
      transport: HID_IO
      chunk_size: 8
      payload_transform: "builtin:dim_timer_write"
"#);
        let mut values = HashMap::new();
        values.insert("dim_timer".to_string(), FieldValue::U8(30)); // 30 minutes → enum 5
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("dim_timer", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(received[0], 0x06);
        assert_eq!(received[1], 0x83);
        assert_eq!(received[2], 5, "30 minutes → device enum 5");
    }

    #[tokio::test]
    async fn write_oled_brightness_sends_level_directly() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  oled_brightness:
    - {name: report_id,       type: uint8, constant: 0x06}
    - {name: command,         type: uint8, constant: 0x85}
    - {name: oled_brightness, type: uint8, range: [1, 10]}
apis:
  oled_brightness:
    write: {transport: HID_IO, chunk_size: 8}
"#);
        let mut values = HashMap::new();
        values.insert("oled_brightness".to_string(), FieldValue::U8(7));
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("oled_brightness", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(&received[..3], &[0x06, 0x85, 7]);
    }

    // ── E6-S7: Bluetooth startup and call behavior ────────────────────────────

    #[tokio::test]
    async fn write_bluetooth_startup_sends_correct_bytes() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  bluetooth_startup:
    - {name: report_id,        type: uint8, constant: 0x06}
    - {name: command,          type: uint8, constant: 0xB2}
    - {name: bt_power_default, type: uint8, range: [0, 1]}
apis:
  bluetooth_startup:
    write: {transport: HID_IO, chunk_size: 8}
"#);
        let mut values = HashMap::new();
        values.insert("bt_power_default".to_string(), FieldValue::U8(1));
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("bluetooth_startup", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(&received[..3], &[0x06, 0xB2, 1]);
    }

    #[tokio::test]
    async fn write_bt_call_default_sends_correct_bytes() {
        let (engine_fd, peer_fd) = make_pair();
        let config = cfg(r#"
structs:
  bt_call_default:
    - {name: report_id,       type: uint8, constant: 0x06}
    - {name: command,         type: uint8, constant: 0xB3}
    - {name: bt_call_default, type: uint8, range: [0, 2]}
apis:
  bt_call_default:
    write: {transport: HID_IO, chunk_size: 8}
"#);
        let mut values = HashMap::new();
        values.insert("bt_call_default".to_string(), FieldValue::U8(2));
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.write_api_direct("bt_call_default", values).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        let received = peer
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(&received[..3], &[0x06, 0xB3, 2]);
    }

    // ── E6-S9: save_to_flash via shutdown lifecycle ───────────────────────────

    #[tokio::test]
    async fn shutdown_lifecycle_sends_save_to_flash() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("device-configs");
        let nova_path = dir.join("nova_pro_wireless.yaml");
        if !nova_path.exists() {
            return; // skip when device-configs not present
        }
        let config = device_config::load(&nova_path, &[dir.as_path()])
            .expect("nova_pro_wireless.yaml must load");

        let (engine_fd, peer_fd) = make_pair();
        let task = tokio::spawn(async move {
            let mut s = DeviceSession::new(config, engine_fd).expect("from_fd");
            s.run_lifecycle_hook("shutdown").await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");
        // shutdown: disable_chatmix (0x49 0x00), disable_sonar (0x8D 0x00), save_to_flash (0x09)
        let mut save_found = false;
        for _ in 0..3 {
            let received = tokio::time::timeout(
                Duration::from_millis(500),
                peer.read_interrupt(Duration::from_millis(500)),
            )
            .await
            .expect("timed out waiting for shutdown command")
            .expect("read failed");
            if received[0] == 0x06 && received[1] == 0x09 {
                save_found = true;
                break;
            }
        }
        assert!(
            save_found,
            "save_to_flash (0x06 0x09) must be sent during shutdown"
        );

        let _ = tokio::time::timeout(Duration::from_millis(500), task).await;
    }

    // ── E1-S8: side-effect events forwarded through channel ──────────────────

    #[tokio::test]
    async fn side_effect_sync_all_events_forwarded_to_channel() {
        use device_config::sync_dispatcher::EventValue;

        let (engine_fd, peer_fd) = make_pair();
        let session = DeviceSession::new(
            cfg(r#"
structs:
  status_settings:
    outgoing:
      - {name: report_id,    type: uint8, constant: 0x00}
      - {name: command,      type: uint8, constant: 0xB0}
    incoming:
      - {name: report_id,    type: uint8, constant: 0x00}
      - {name: command,      type: uint8, constant: 0xB0}
      - {name: status_field, type: uint8}
apis:
  status_settings:
    read: {transport: HID_IO, chunk_size: 8}
sync_events:
  0x10:
    emit: trigger_event
    fields: []
    side_effects:
      - call: sync_all
sync_read:
  - struct: status_settings
    maps:
      - {emit: status_field_changed, field: status_field}
"#),
            engine_fd,
        )
        .expect("from_fd");

        let (event_tx, mut event_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel::<DeviceCommand>(8);

        let task = tokio::spawn(async move {
            let mut s = session;
            s.run_event_loop_with_commands(event_tx, cmd_rx).await
        });

        let mut peer = HidTransport::from_fd(peer_fd).expect("from_fd");

        // Send trigger event 0x10 — has sync_all as side effect.
        let mut trigger = vec![0u8; 8];
        trigger[1] = 0x10;
        peer.write_interrupt(&trigger).await.unwrap();

        // Engine runs sync_all: sends a read request for status_settings.
        let _req = tokio::time::timeout(
            Duration::from_millis(500),
            peer.read_interrupt(Duration::from_millis(500)),
        )
        .await
        .expect("engine should send sync_read request")
        .expect("read ok");

        // Serve the sync_read response with status_field = 99.
        let mut resp = vec![0u8; 8];
        resp[0] = 0x00; // report_id
        resp[1] = 0xB0; // command
        resp[2] = 99; // status_field
        peer.write_interrupt(&resp).await.unwrap();

        // Expect trigger_event first, then status_field_changed from sync_all.
        let ev1 = tokio::time::timeout(Duration::from_millis(500), event_rx.recv())
            .await
            .expect("timed out waiting for trigger_event")
            .expect("channel closed");
        let ev2 = tokio::time::timeout(Duration::from_millis(500), event_rx.recv())
            .await
            .expect("timed out waiting for status_field_changed — side effect events not forwarded")
            .expect("channel closed");

        let signals = [ev1.signal.as_str(), ev2.signal.as_str()];
        assert!(
            signals.contains(&"trigger_event"),
            "trigger_event missing: {signals:?}"
        );
        assert!(
            signals.contains(&"status_field_changed"),
            "status_field_changed missing — side effect events are being discarded: {signals:?}"
        );

        let sync_ev = if ev1.signal == "status_field_changed" {
            &ev1
        } else {
            &ev2
        };
        assert_eq!(
            sync_ev.fields.get("status_field"),
            Some(&EventValue::Field(FieldValue::U8(99))),
            "status_field value mismatch"
        );

        drop(event_rx);
        drop(peer);
        let _ = tokio::time::timeout(Duration::from_millis(500), task).await;
    }
}
