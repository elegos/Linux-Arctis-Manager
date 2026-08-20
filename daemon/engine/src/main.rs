mod audio;
mod dbus;
mod eq;
mod device_persistence;
mod device_session;
mod engine_error;
mod general_settings;
mod hidraw_client;
mod hotplug;
mod state;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::time::Duration;

use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use device_config::codec::FieldValue;
use device_config::sync_dispatcher::{EmitEvent, EventValue};
use device_config::DeviceConfig;
use device_session::DeviceSession;
use engine_error::EngineError;
use hotplug::DeviceInfo;
use state::{AppState, DeviceCommand, DeviceEntry, SignalEvent};

// ── Config loading ────────────────────────────────────────────────────────────

/// System-level data directory baked in at compile time via `LAM_DATADIR`.
/// `None` when building without `make install` (dev builds, unit tests).
const SYSTEM_DATADIR: Option<&str> = option_env!("LAM_DATADIR");

/// Load `*.yaml` files from every directory in `dirs`.
/// All dirs are passed as search paths to the DSL loader so that `extends:`
/// references can cross directory boundaries (e.g. a user override that extends
/// a system base file).  Earlier entries in `dirs` have higher priority: when
/// `find_config` stops at the first PID (and interface, when available) match.
pub fn load_configs_from_dirs(dirs: &[&Path]) -> Vec<Arc<DeviceConfig>> {
    let mut configs = Vec::new();
    for &dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            match device_config::load(&path, dirs) {
                Ok(cfg) => configs.push(Arc::new(cfg)),
                Err(e) => warn!("skipping {}: {e}", path.display()),
            }
        }
    }
    configs
}

/// Return the first config whose PID matches `pid`.
///
/// When `interface_num` is provided, a config that specifies a matching
/// `command_interface` is preferred.  If no exact match exists (e.g. when the
/// headset is off and only a non-command interface is enumerated by udev), the
/// check falls back to PID-only so that the reconnect loop can still start and
/// will succeed once the headset powers on.
fn find_config(
    configs: &[Arc<DeviceConfig>],
    pid: u16,
    interface_num: Option<u8>,
) -> Option<&Arc<DeviceConfig>> {
    let pid_matches = |c: &&Arc<DeviceConfig>| {
        c.device
            .as_ref()
            .and_then(|d| d.variants.as_ref())
            .map(|vs| vs.iter().any(|v| v.product_id == pid))
            .unwrap_or(false)
    };

    // Preferred: PID match AND command-interface match (or no interface info).
    let preferred = configs.iter().find(|c| {
        if !pid_matches(c) {
            return false;
        }
        if let (Some(iface), Some(hid)) = (
            interface_num,
            c.device.as_ref().and_then(|d| d.hid.as_ref()),
        ) {
            if let Some(cmd) = &hid.command_interface {
                return cmd.interface == iface;
            }
        }
        true
    });

    if preferred.is_some() {
        return preferred;
    }

    // Fallback: PID match only — wrong interface, but better than skipping
    // the device entirely (reconnect loop handles device_init failures).
    if interface_num.is_some() {
        if let Some(cfg) = configs.iter().find(|c| pid_matches(c)) {
            warn!(
                "PID {:#06x} found on interface {:?}, expected command interface; \
                 starting task anyway — device_init will retry when headset is ready",
                pid, interface_num
            );
            return Some(cfg);
        }
    }

    None
}

/// Return the path to the hidraw-helper socket.
pub fn helper_sock_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("lam-hidraw-helper.sock")
}

/// Return the user-level directory where device YAML configs are stored.
pub fn user_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("arctis_manager/devices")
}

/// Return the system-level config directory compiled in at build time, if any.
pub fn system_config_dir() -> Option<PathBuf> {
    SYSTEM_DATADIR.map(|d| PathBuf::from(d).join("devices"))
}

/// Build the ordered list of config search directories.
/// User dir is first (highest priority); system dir is appended when compiled in.
pub fn config_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![user_config_dir()];
    if let Some(sys) = system_config_dir() {
        dirs.push(sys);
    }
    dirs
}

/// Base directory for all user-level Arctis Manager config files.
/// `~/.config/arctis_manager` (respects XDG_CONFIG_HOME).
pub fn user_settings_base_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("arctis_manager")
}

/// Path to the persisted general settings YAML file.
pub fn general_settings_path() -> PathBuf {
    user_settings_base_dir().join("general_settings.yaml")
}

// ── Per-device task ───────────────────────────────────────────────────────────

/// Initialise and run the event loop for one device.  Registers the device in
/// `app_state`, emits hotplug signals, and forwards `EmitEvent`s to the D-Bus
/// state map.
///
/// Runs a reconnect loop: if the headset is off when the daemon starts, or
/// powers off while running, the loop retries `device_init` automatically.
/// The task runs until aborted by the hotplug Removed handler (dongle removed).
async fn run_device(
    info: DeviceInfo,
    config: Arc<DeviceConfig>,
    helper_sock: PathBuf,
    app_state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
) {
    let path_str = info.hidraw_path.to_string_lossy().to_string();
    info!("monitoring {path_str} (PID={:#06x})", info.pid);

    let friendly_name = config
        .device
        .as_ref()
        .and_then(|d| d.variants.as_ref())
        .and_then(|vs| vs.iter().find(|v| v.product_id == info.pid))
        .and_then(|v| v.name.clone())
        .unwrap_or_else(|| path_str.clone());

    let capabilities: Vec<String> = config
        .device
        .as_ref()
        .and_then(|d| d.capabilities.as_ref())
        .cloned()
        .unwrap_or_default();

    // Register the device once (the dongle is present).  A placeholder
    // cmd_tx is used until the first successful session replaces it.
    let (placeholder_tx, _) = mpsc::channel::<DeviceCommand>(1);
    {
        let mut s = app_state.lock().await;
        s.devices.insert(
            info.hidraw_path.clone(),
            DeviceEntry {
                config: Arc::clone(&config),
                vid: info.vid,
                pid: info.pid,
                name: friendly_name.clone(),
                capabilities: capabilities.clone(),
                status: HashMap::new(),
                cmd_tx: placeholder_tx,
            },
        );
    }
    let _ = signal_tx.send(SignalEvent::DeviceConnected {
        pid: info.pid,
        name: friendly_name.clone(),
        capabilities,
    });

    // Reconnection loop: retries whenever the headset is powered off or
    // disconnects.  Exits only when the task is aborted (dongle removed).
    'reconnect: loop {
        let fd = match hidraw_client::request_fd(&helper_sock, &path_str).await {
            Ok(fd) => fd,
            Err(e) => {
                warn!("fd request failed for {path_str}: {e}");
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue 'reconnect;
            }
        };

        let mut session = DeviceSession::new((*config).clone(), fd);

        // Inner loop: keep the fd open and wait reactively for the headset.
        // On timeout (headset off) we listen for any async HID event from the
        // dongle instead of sleeping; the wireless-connection-changed report
        // arrives as soon as the headset powers on, triggering an immediate
        // device_init retry without a fixed polling interval.
        let init_events = 'init: loop {
            match session.device_init().await {
                Ok(events) => {
                    info!("headset connected: {friendly_name}");
                    break 'init events;
                }
                Err(EngineError::Io(ref e))
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.raw_os_error() == Some(110) =>
                {
                    // Headset not responding.  Block on the open fd waiting for
                    // any async notification (typically 0xB5 wireless-connection-
                    // changed when the headset powers on).  This is reactive:
                    // we wake the moment the dongle speaks, not on a timer.
                    info!("headset not ready, waiting for wireless event on {path_str}...");
                    match session.read_any_report(Duration::from_secs(30)).await {
                        Ok(report) => {
                            debug!(
                                "async event received (cmd={:#04x}), retrying device_init",
                                report.get(1).copied().unwrap_or(0)
                            );
                        }
                        Err(EngineError::Io(ref e2))
                            if e2.kind() != std::io::ErrorKind::TimedOut =>
                        {
                            // Real transport error — need a fresh fd.
                            continue 'reconnect;
                        }
                        Err(_) => {} // 30-second timeout: try device_init again
                    }
                }
                Err(e) => {
                    info!("headset not ready on {path_str} ({e}), retrying");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue 'reconnect;
                }
            }
        };

        // Populate the status map with the initial snapshot and signal D-Bus.
        {
            let mut s = app_state.lock().await;
            if let Some(entry) = s.devices.get_mut(&info.hidraw_path) {
                for ev in &init_events {
                    for (field, val) in &ev.fields {
                        let dt = ev.display_types.get(field).map(String::as_str);
                        entry
                            .status
                            .insert(field.clone(), state::event_value_to_json(val, dt));
                    }
                }
            }
        }
        let _ = signal_tx.send(SignalEvent::StatusChanged);

        // Redirect default sink to Arctis_Media on headset connect, if enabled.
        {
            let redirect = app_state
                .lock()
                .await
                .general_settings
                .redirect_audio_on_connect;
            if redirect {
                audio::set_default_sink(audio::MEDIA_SINK).await;
            }
        }

        // Create virtual audio sinks and apply the initial chatmix balance.
        // Wrapped in Arc<Mutex> so the event-forwarding task can drive the
        // audio lifecycle directly from radio_connection_status events.
        let audio_shared = Arc::new(Mutex::new(match audio::setup_sinks().await {
            Ok(setup) => {
                if let (Some(game), Some(chat)) = chatmix_from_events(&init_events) {
                    audio::set_chatmix(game, chat).await;
                }
                Some(setup)
            }
            Err(e) => {
                warn!("audio setup failed for {path_str}: {e}");
                None
            }
        }));

        // Fresh command channel per session.  Buffer sized to hold all re-apply
        // commands before the event loop starts consuming them.
        let (cmd_tx, cmd_rx) = mpsc::channel::<DeviceCommand>(64);

        // Re-apply persisted device settings before the event loop starts, so
        // the device state matches what the user last configured.
        let settings_file =
            device_persistence::settings_file_path(&user_settings_base_dir(), info.vid, info.pid);
        let overrides = device_persistence::load_device_settings(&settings_file);
        if !overrides.is_empty() {
            info!(
                "re-applying {} persisted setting(s) for PID={:#06x}",
                overrides.len(),
                info.pid
            );
            for (field, json_val) in &overrides {
                let raw = serde_json::to_string(json_val).unwrap_or_default();
                if let Some(api_name) = dbus::find_api_for_field(&config, field) {
                    if let Some(fv) = dbus::parse_setting_value(&config, &api_name, field, &raw) {
                        let mut values = HashMap::new();
                        values.insert(field.clone(), fv);
                        let _ = cmd_tx
                            .send(DeviceCommand::WriteApi { api_name, values })
                            .await;
                    }
                }
            }
        }

        {
            let mut s = app_state.lock().await;
            if let Some(entry) = s.devices.get_mut(&info.hidraw_path) {
                entry.cmd_tx = cmd_tx;
            }
        }

        let (event_tx, mut event_rx) = mpsc::channel::<EmitEvent>(64);

        // Forward EmitEvents to the state map; manage audio lifecycle from
        // radio_connection_status; apply chatmix on slider changes.
        let state_for_events = Arc::clone(&app_state);
        let signal_tx_clone = signal_tx.clone();
        let hidraw_path_clone = info.hidraw_path.clone();
        let audio_for_task = Arc::clone(&audio_shared);
        tokio::spawn(async move {
            while let Some(ev) = event_rx.recv().await {
                info!(signal = %ev.signal, fields = ?ev.fields, "sync event");
                let mut chatmix_changed = false;
                let mut radio_status: Option<String> = None;
                {
                    let mut s = state_for_events.lock().await;
                    if let Some(entry) = s.devices.get_mut(&hidraw_path_clone) {
                        for (field, val) in &ev.fields {
                            let dt = ev.display_types.get(field).map(String::as_str);
                            entry
                                .status
                                .insert(field.clone(), state::event_value_to_json(val, dt));
                            match field.as_str() {
                                "chatmix_game" | "chatmix_chat" => chatmix_changed = true,
                                "radio_connection_status" => {
                                    if let EventValue::Str(s) = val {
                                        radio_status = Some(s.clone());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                let _ = signal_tx_clone.send(SignalEvent::StatusChanged);

                // Drive audio sink lifecycle from the wireless connection event.
                // Guards are never held across .await — take/set value, drop, then await.
                if let Some(ref status) = radio_status {
                    if status.contains("NOT_CONNECTED") || status == "DISCONNECTED" {
                        let setup = audio_for_task.lock().await.take();
                        if let Some(s) = setup {
                            info!("headset wireless off: removing virtual sinks");
                            audio::teardown_sinks(s).await;
                        }
                        // Redirect to user-chosen sink on wireless disconnect.
                        let (do_redirect, target) = {
                            let s = state_for_events.lock().await;
                            (
                                s.general_settings.redirect_audio_on_disconnect,
                                s.general_settings
                                    .redirect_audio_on_disconnect_device
                                    .clone(),
                            )
                        };
                        if do_redirect {
                            if let Some(ref sink) = target {
                                audio::set_default_sink(sink).await;
                            }
                        }
                    } else if status == "PAIRED_CONNECTED" || status == "CONNECTED" {
                        let needs = audio_for_task.lock().await.is_none();
                        if needs {
                            match audio::setup_sinks().await {
                                Ok(s) => {
                                    info!("headset wireless on: virtual sinks created");
                                    *audio_for_task.lock().await = Some(s);
                                }
                                Err(e) => warn!("audio setup on reconnect failed: {e}"),
                            }
                        }
                    }
                }

                if chatmix_changed {
                    let (game, chat) = {
                        let s = state_for_events.lock().await;
                        let e = s.devices.get(&hidraw_path_clone);
                        let g = e
                            .and_then(|e| e.status.get("chatmix_game"))
                            .and_then(|v| v["value"].as_u64())
                            .map(|v| v as u8);
                        let c = e
                            .and_then(|e| e.status.get("chatmix_chat"))
                            .and_then(|v| v["value"].as_u64())
                            .map(|v| v as u8);
                        (g, c)
                    };
                    if let (Some(g), Some(c)) = (game, chat) {
                        audio::set_chatmix(g, c).await;
                    }
                }
            }
        });

        match session.run_event_loop_with_commands(event_tx, cmd_rx).await {
            Err(EngineError::Io(ref io_e))
                if io_e.kind() == std::io::ErrorKind::UnexpectedEof
                    || io_e.kind() == std::io::ErrorKind::BrokenPipe =>
            {
                info!("headset disconnected: {friendly_name}");
            }
            Err(e) => error!("event loop error for {path_str}: {e}"),
            Ok(()) => info!("headset disconnected: {friendly_name}"),
        }

        // Tear down any sinks still alive (e.g. hidraw EOF while headset was on).
        if let Some(setup) = audio_shared.lock().await.take() {
            audio::teardown_sinks(setup).await;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn cleanup_device(
    app_state: &Arc<Mutex<AppState>>,
    path: &PathBuf,
    pid: u16,
    signal_tx: &broadcast::Sender<SignalEvent>,
) {
    app_state.lock().await.devices.remove(path);
    let _ = signal_tx.send(SignalEvent::DeviceDisconnected { pid });
}

/// Extract the initial chatmix_game and chatmix_chat values from device init events.
fn chatmix_from_events(events: &[EmitEvent]) -> (Option<u8>, Option<u8>) {
    let mut game = None;
    let mut chat = None;
    for ev in events {
        for (field, val) in &ev.fields {
            if let EventValue::Field(FieldValue::U8(v)) = val {
                match field.as_str() {
                    "chatmix_game" => game = Some(*v),
                    "chatmix_chat" => chat = Some(*v),
                    _ => {}
                }
            }
        }
    }
    (game, chat)
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("lam-daemon {}", env!("LAM_VERSION"));

    let cfg_dirs = config_dirs();
    let dir_refs: Vec<&Path> = cfg_dirs.iter().map(PathBuf::as_path).collect();
    let configs = load_configs_from_dirs(&dir_refs);
    if configs.is_empty() {
        warn!(
            "no device configs found in [{}]",
            cfg_dirs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        info!("loaded {} device config(s)", configs.len());
    }

    let helper_sock = helper_sock_path();

    let gs_path = general_settings_path();
    let gs = general_settings::GeneralSettings::load_from_file(&gs_path);
    info!("general settings loaded from {}", gs_path.display());

    let app_state = Arc::new(Mutex::new(AppState {
        configs: configs.clone(),
        devices: HashMap::new(),
        config_dirs: cfg_dirs,
        general_settings: gs,
        general_settings_path: gs_path,
    }));

    let (signal_tx, signal_rx) = broadcast::channel::<SignalEvent>(64);

    // Start D-Bus service.  On failure, continue without it (headless / test env).
    let _dbus_conn = match dbus::start_dbus_service(
        Arc::clone(&app_state),
        signal_rx,
        signal_tx.clone(),
        user_settings_base_dir(),
    )
    .await
    {
        Ok(c) => {
            info!("D-Bus service registered");
            Some(c)
        }
        Err(e) => {
            warn!("D-Bus service unavailable: {e}, continuing without D-Bus");
            None
        }
    };

    run_main_loop(configs, helper_sock, app_state, signal_tx).await;
}

async fn run_main_loop(
    configs: Vec<Arc<DeviceConfig>>,
    helper_sock: PathBuf,
    app_state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
) {
    let existing = match hotplug::scan_existing(&[]) {
        Ok(devs) => devs,
        Err(e) => {
            warn!("udev enumeration failed: {e}");
            vec![]
        }
    };

    let mut tasks: HashMap<PathBuf, JoinHandle<()>> = HashMap::new();

    for dev in existing {
        info!(
            "device at startup: {} (VID={:#06x} PID={:#06x} iface={:?})",
            dev.hidraw_path.display(),
            dev.vid,
            dev.pid,
            dev.interface_num
        );
        if let Some(cfg) = find_config(&configs, dev.pid, dev.interface_num) {
            let cfg = Arc::clone(cfg);
            let sock = helper_sock.clone();
            let path = dev.hidraw_path.clone();
            let state = Arc::clone(&app_state);
            let stx = signal_tx.clone();
            let handle = tokio::spawn(run_device(dev, cfg, sock, state, stx));
            tasks.insert(path, handle);
        } else {
            info!("no config for PID {:#06x}, skipping", dev.pid);
        }
    }

    let (tx, mut rx) = mpsc::channel::<hotplug::HotplugEvent>(16);
    tokio::select! {
        res = hotplug::watch(vec![], tx) => {
            if let Err(e) = res {
                error!("hotplug watch failed: {e}");
            }
        }
        _ = async {
            while let Some(event) = rx.recv().await {
                match event {
                    hotplug::HotplugEvent::Added(dev) => {
                        info!(
                            "hotplug add: {} (PID={:#06x})",
                            dev.hidraw_path.display(), dev.pid
                        );
                        if let Some(cfg) = find_config(&configs, dev.pid, dev.interface_num) {
                            let cfg = Arc::clone(cfg);
                            let sock = helper_sock.clone();
                            let path = dev.hidraw_path.clone();
                            let state = Arc::clone(&app_state);
                            let stx = signal_tx.clone();
                            let handle = tokio::spawn(run_device(dev, cfg, sock, state, stx));
                            tasks.insert(path, handle);
                        } else {
                            info!("no config for PID {:#06x}", dev.pid);
                        }
                    }
                    hotplug::HotplugEvent::Removed(dev) => {
                        info!(
                            "hotplug remove: {} (PID={:#06x})",
                            dev.hidraw_path.display(), dev.pid
                        );
                        if let Some(handle) = tasks.remove(&dev.hidraw_path) {
                            handle.abort();
                        }
                        // run_device loops forever; clean up state here on dongle removal.
                        cleanup_device(&app_state, &dev.hidraw_path, dev.pid, &signal_tx).await;
                    }
                }
            }
        } => {}
    }
}
