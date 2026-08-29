mod audio;
mod dbus;
mod device_persistence;
mod device_session;
mod engine_error;
mod eq;
mod eq_manager;
mod focus_monitor;
mod general_settings;
mod hidraw_client;
mod hotplug;
mod ladspa_util;
mod mic_router;
mod nc_config;
mod nc_manager;
mod state;
mod stream_monitor;
mod vc_base_models;
mod vc_calibration;
mod vc_config;
mod vc_hf_client;
mod vc_ladspa_chain;
mod vc_models;
mod vc_rvc_config;

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
            let iface_s = interface_num.map_or_else(|| "?".to_string(), |i| i.to_string());
            warn!(
                "PID {:#06x} on interface {iface_s} matched by PID only \
                 (expected command interface); starting task anyway — \
                 device_init will retry when headset is ready",
                pid
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
    audio_shared: Arc<Mutex<Option<audio::AudioSetup>>>,
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

        let mut session = match DeviceSession::new((*config).clone(), fd) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to open transport for {path_str}: {e}");
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue 'reconnect;
            }
        };

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

        // Push a SettingsChanged so the Device settings panel shows real current
        // values (status is now populated, so build_settings_json has actuals).
        {
            let s = app_state.lock().await;
            let json = dbus::build_settings_json(&s);
            let _ = signal_tx.send(SignalEvent::SettingsChanged { json });
        }

        // Create virtual audio sinks and apply the initial chatmix balance.
        // Must run before redirect so Arctis_Media exists when set_default_sink
        // is called.
        {
            let mut guard = audio_shared.lock().await;
            if guard.is_none() {
                match audio::setup_sinks().await {
                    Ok(setup) => {
                        if let (Some(game), Some(chat)) = chatmix_from_events(&init_events) {
                            audio::set_chatmix(game, chat).await;
                        }
                        *guard = Some(setup);
                    }
                    Err(e) => warn!("audio setup failed for {path_str}: {e}"),
                }
            }
        }

        // Redirect default sink to Arctis_Media on headset connect, if enabled.
        // Runs after setup_sinks so the sink exists.
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
            // Group persisted fields by their API struct so that multi-field
            // structs (e.g. stream_mix: stream_main + stream_aux + stream_mic)
            // are sent as a single WriteApi call.  Sending them one-by-one
            // causes codec "missing value" errors because the codec requires
            // every non-constant field to be present.
            let mut api_groups: HashMap<String, HashMap<String, device_config::codec::FieldValue>> =
                HashMap::new();
            for (field, json_val) in &overrides {
                let raw = serde_json::to_string(json_val).unwrap_or_default();
                if let Some(api_name) = dbus::find_api_for_field(&config, field) {
                    if let Some(fv) = dbus::parse_setting_value(&config, &api_name, field, &raw) {
                        api_groups
                            .entry(api_name)
                            .or_default()
                            .insert(field.clone(), fv);
                    }
                }
            }
            for (api_name, partial) in api_groups {
                // Fill any sibling fields not in the persisted map with their
                // range minimum (or 0) so the codec receives a complete struct.
                let values = dbus::build_write_values_with_defaults(&config, &api_name, partial);
                let _ = cmd_tx
                    .send(DeviceCommand::WriteApi { api_name, values })
                    .await;
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
            // Edge-detect state: the device re-emits full status (including
            // radio_connection_status and chatmix_*) on every periodic poll,
            // not only on change. Without tracking the last-seen value here,
            // the audio lifecycle/redirect logic below would re-fire every
            // poll cycle (~10s) even while nothing changed, spamming pactl
            // calls against sinks that were already torn down.
            let mut last_radio_status: Option<String> = None;
            let mut last_chatmix: Option<(u8, u8)> = None;
            while let Some(ev) = event_rx.recv().await {
                debug!(signal = %ev.signal, fields = ?ev.fields, "sync event");
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
                // Only react on an actual transition — the device re-sends the
                // same radio_connection_status on every periodic poll, and
                // acting on every poll spams pactl against already-torn-down
                // sinks and repeatedly redirects the default sink.
                // Guards are never held across .await — take/set value, drop, then await.
                if let Some(ref status) = radio_status {
                    let changed = last_radio_status.as_deref() != Some(status.as_str());
                    last_radio_status = Some(status.clone());
                    if changed {
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

                                        // Redirect default sink to Arctis_Media on
                                        // wireless (re)connect, mirroring the
                                        // cold-start redirect. Without this, a
                                        // reconnect after the initial session start
                                        // (e.g. headset powered on later) never
                                        // switches the default sink.
                                        let redirect = state_for_events
                                            .lock()
                                            .await
                                            .general_settings
                                            .redirect_audio_on_connect;
                                        if redirect {
                                            audio::set_default_sink(audio::MEDIA_SINK).await;
                                        }
                                    }
                                    Err(e) => warn!("audio setup on reconnect failed: {e}"),
                                }
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
                        if last_chatmix != Some((g, c)) {
                            last_chatmix = Some((g, c));
                            audio::set_chatmix(g, c).await;
                        }
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
    // Accept --log-level=<level> or --log-level <level>; falls back to RUST_LOG, then "info".
    let log_level = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|w| {
            if w[0] == "--log-level" {
                Some(w[1].clone())
            } else {
                w[0].strip_prefix("--log-level=").map(str::to_owned)
            }
        });

    let filter = if let Some(ref level) = log_level {
        tracing_subscriber::EnvFilter::new(level)
    } else {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

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

    // Global audio shared state: set when a device session sets up virtual sinks;
    // cleared on disconnect.  Shared with the EQ D-Bus interface for routing.
    let audio_shared: Arc<Mutex<Option<audio::AudioSetup>>> = Arc::new(Mutex::new(None));

    // NC and mic-router shared state.
    let nc_runtime: Arc<Mutex<nc_manager::NcRuntime>> =
        Arc::new(Mutex::new(nc_manager::NcRuntime::new()));
    let mic_router: Arc<Mutex<mic_router::MicRouterState>> =
        Arc::new(Mutex::new(mic_router::MicRouterState::new()));

    // Start D-Bus service.  On failure, continue without it (headless / test env).
    let _dbus_conn = match dbus::start_dbus_service(
        Arc::clone(&app_state),
        signal_rx,
        signal_tx.clone(),
        user_settings_base_dir(),
        Arc::clone(&audio_shared),
        Arc::clone(&nc_runtime),
        Arc::clone(&mic_router),
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

    run_main_loop(
        configs,
        helper_sock,
        app_state,
        signal_tx,
        audio_shared,
        nc_runtime,
        mic_router,
    )
    .await;
}

/// Keep only devices that are useful to start a task for.
///
/// If the command interface for a given PID is present in the list, discard all
/// other interfaces for that PID — they would only produce fallback matches and
/// repeated device_init timeouts.  If the command interface is absent (headset
/// off, only dongle enumerated), keep them so the reconnect loop can wait for it.
fn filter_to_command_interfaces(
    devs: &[hotplug::DeviceInfo],
    configs: &[Arc<DeviceConfig>],
) -> Vec<hotplug::DeviceInfo> {
    use std::collections::HashSet;

    // Collect (pid, command_interface) pairs that ARE present in the list.
    let present_cmd: HashSet<(u16, u8)> = devs
        .iter()
        .flat_map(|d| {
            configs.iter().find_map(|c| {
                let pid_match = c
                    .device
                    .as_ref()
                    .and_then(|dev| dev.variants.as_ref())
                    .map(|vs| vs.iter().any(|v| v.product_id == d.pid))
                    .unwrap_or(false);
                if !pid_match {
                    return None;
                }
                let cmd = c
                    .device
                    .as_ref()
                    .and_then(|dev| dev.hid.as_ref())
                    .and_then(|h| h.command_interface.as_ref())
                    .map(|ci| ci.interface)?;
                if d.interface_num == Some(cmd) {
                    Some((d.pid, cmd))
                } else {
                    None
                }
            })
        })
        .collect();

    devs.iter()
        .filter(|d| {
            // Find the expected command interface for this PID (if any).
            let cmd_iface = configs.iter().find_map(|c| {
                let pid_match = c
                    .device
                    .as_ref()
                    .and_then(|dev| dev.variants.as_ref())
                    .map(|vs| vs.iter().any(|v| v.product_id == d.pid))
                    .unwrap_or(false);
                if !pid_match {
                    return None;
                }
                c.device
                    .as_ref()
                    .and_then(|dev| dev.hid.as_ref())
                    .and_then(|h| h.command_interface.as_ref())
                    .map(|ci| ci.interface)
            });

            match cmd_iface {
                // No command_interface in config — keep the device as-is.
                None => true,
                // Command interface defined and THIS device is it — keep.
                Some(cmd) if d.interface_num == Some(cmd) => true,
                // Command interface defined, this device is NOT it, but the
                // correct one IS present — skip to avoid a useless task.
                Some(cmd) if present_cmd.contains(&(d.pid, cmd)) => false,
                // Command interface defined but not yet enumerated — keep for
                // the reconnect loop (headset off, only dongle visible).
                _ => true,
            }
        })
        .cloned()
        .collect()
}

async fn run_main_loop(
    configs: Vec<Arc<DeviceConfig>>,
    helper_sock: PathBuf,
    app_state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
    audio_shared: Arc<Mutex<Option<audio::AudioSetup>>>,
    nc_runtime: Arc<Mutex<nc_manager::NcRuntime>>,
    mic_router: Arc<Mutex<mic_router::MicRouterState>>,
) {
    let existing = match hotplug::scan_existing(&[]) {
        Ok(devs) => devs,
        Err(e) => {
            warn!("udev enumeration failed: {e}");
            vec![]
        }
    };

    // For each PID, if the command interface is already present in the scanned
    // list, skip non-command interfaces — they would only produce fallback matches
    // and useless device_init timeouts.
    let existing = filter_to_command_interfaces(&existing, &configs);

    // (pid, interface_num, task_handle) — interface_num lets us identify and
    // abort wrong-interface fallback tasks when the correct one arrives later.
    let mut tasks: HashMap<PathBuf, (u16, Option<u8>, JoinHandle<()>)> = HashMap::new();

    for dev in existing {
        let iface_s = dev
            .interface_num
            .map_or_else(|| "?".to_string(), |i| i.to_string());
        if let Some(cfg) = find_config(&configs, dev.pid, dev.interface_num) {
            let name = cfg
                .device
                .as_ref()
                .and_then(|d| d.variants.as_ref())
                .and_then(|vs| vs.iter().find(|v| v.product_id == dev.pid))
                .and_then(|v| v.name.as_deref())
                .unwrap_or("unknown");
            info!(
                "device at startup: {} PID={:#06x} iface={} ({})",
                dev.hidraw_path.display(),
                dev.pid,
                iface_s,
                name
            );
            let cfg = Arc::clone(cfg);
            let sock = helper_sock.clone();
            let path = dev.hidraw_path.clone();
            let state = Arc::clone(&app_state);
            let stx = signal_tx.clone();
            let aud = Arc::clone(&audio_shared);
            let (pid, iface_num) = (dev.pid, dev.interface_num);
            let handle = tokio::spawn(run_device(dev, cfg, sock, state, stx, aud));
            tasks.insert(path, (pid, iface_num, handle));
        } else {
            info!(
                "no config for PID={:#06x} iface={}, skipping",
                dev.pid, iface_s
            );
        }
    }

    let (tx, mut rx) = mpsc::channel::<hotplug::HotplugEvent>(16);

    // Install SIGTERM handler once before the select! loop.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| warn!("SIGTERM handler unavailable: {e}"))
        .ok();

    let aud_on_remove = Arc::clone(&audio_shared);

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
                        let iface_s = dev
                            .interface_num
                            .map_or_else(|| "?".to_string(), |i| i.to_string());
                        if let Some(cfg) = find_config(&configs, dev.pid, dev.interface_num) {
                            let cmd_iface = cfg
                                .device
                                .as_ref()
                                .and_then(|d| d.hid.as_ref())
                                .and_then(|h| h.command_interface.as_ref())
                                .map(|ci| ci.interface);

                            let is_correct_iface = cmd_iface
                                .is_none_or(|cmd| dev.interface_num == Some(cmd));

                            if is_correct_iface {
                                // Abort any fallback (wrong-interface) task that was
                                // waiting for this PID's correct interface to appear.
                                let fallbacks: Vec<PathBuf> = tasks
                                    .iter()
                                    .filter(|(_, (pid, iface, _))| {
                                        *pid == dev.pid
                                            && cmd_iface
                                                .is_some_and(|cmd| *iface != Some(cmd))
                                    })
                                    .map(|(k, _)| k.clone())
                                    .collect();
                                for k in fallbacks {
                                    if let Some((_, fi, handle)) = tasks.remove(&k) {
                                        info!(
                                            "aborting fallback task for {:?} iface={:?} \
                                             (correct iface={} now present)",
                                            k,
                                            fi,
                                            cmd_iface.unwrap_or(0)
                                        );
                                        handle.abort();
                                        // Remove the placeholder DeviceEntry that the
                                        // fallback task registered before being aborted,
                                        // otherwise build_status_json may pick it up and
                                        // return "{}" even though the correct interface is
                                        // already running.
                                        app_state.lock().await.devices.remove(&k);
                                    }
                                }
                            } else {
                                // Fallback match: skip if the correct interface is already running.
                                let cmd_running = cmd_iface.is_some_and(|cmd| {
                                    tasks.values().any(|(_, iface, _)| *iface == Some(cmd))
                                });
                                if cmd_running {
                                    info!(
                                        "hotplug add: skipping {} iface={} \
                                         (cmd iface={} already running)",
                                        dev.hidraw_path.display(),
                                        iface_s,
                                        cmd_iface.unwrap_or(0)
                                    );
                                    continue;
                                }
                            }

                            let name = cfg
                                .device
                                .as_ref()
                                .and_then(|d| d.variants.as_ref())
                                .and_then(|vs| vs.iter().find(|v| v.product_id == dev.pid))
                                .and_then(|v| v.name.as_deref())
                                .unwrap_or("unknown");
                            info!(
                                "hotplug add: {} PID={:#06x} iface={} ({})",
                                dev.hidraw_path.display(),
                                dev.pid,
                                iface_s,
                                name
                            );
                            let cfg = Arc::clone(cfg);
                            let sock = helper_sock.clone();
                            let path = dev.hidraw_path.clone();
                            let state = Arc::clone(&app_state);
                            let stx = signal_tx.clone();
                            let aud = Arc::clone(&audio_shared);
                            let (pid, iface_num) = (dev.pid, dev.interface_num);
                            let handle = tokio::spawn(run_device(dev, cfg, sock, state, stx, aud));
                            tasks.insert(path, (pid, iface_num, handle));
                        } else {
                            info!(
                                "no config for PID={:#06x} iface={}, skipping",
                                dev.pid, iface_s
                            );
                        }
                    }
                    hotplug::HotplugEvent::Removed(dev) => {
                        info!(
                            "hotplug remove: {} (PID={:#06x})",
                            dev.hidraw_path.display(), dev.pid
                        );
                        if let Some((_, _, handle)) = tasks.remove(&dev.hidraw_path) {
                            handle.abort();
                        }
                        // The aborted task may not have reached its own teardown;
                        // take and destroy the sinks here unconditionally.
                        if let Some(setup) = aud_on_remove.lock().await.take() {
                            info!("dongle removed: removing virtual audio sinks");
                            audio::teardown_sinks(setup).await;
                        }
                        mic_router::teardown(&mut *mic_router.lock().await).await;
                        nc_manager::teardown_nc(&mut *nc_runtime.lock().await).await;
                        cleanup_device(&app_state, &dev.hidraw_path, dev.pid, &signal_tx).await;
                    }
                }
            }
        } => {}
        _ = tokio::signal::ctrl_c() => {
            info!("SIGINT received, shutting down");
        }
        _ = async {
            match sigterm.as_mut() {
                Some(s) => { s.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => {
            info!("SIGTERM received, shutting down");
        }
    }

    // Tear down virtual audio sinks on any exit path.
    if let Some(setup) = audio_shared.lock().await.take() {
        info!("daemon exit: removing virtual audio sinks");
        audio::teardown_sinks(setup).await;
    }
    mic_router::teardown(&mut *mic_router.lock().await).await;
    nc_manager::teardown_nc(&mut *nc_runtime.lock().await).await;
}
