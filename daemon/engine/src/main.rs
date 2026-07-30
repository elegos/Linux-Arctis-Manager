mod dbus;
mod device_session;
mod engine_error;
mod hidraw_client;
mod hotplug;
mod state;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use device_config::sync_dispatcher::EmitEvent;
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
/// `find_config` walks the Vec it stops at the first PID match.
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

/// Return the first config whose `device.variants` includes `pid`.
fn find_config(configs: &[Arc<DeviceConfig>], pid: u16) -> Option<&Arc<DeviceConfig>> {
    configs.iter().find(|c| {
        c.device
            .as_ref()
            .and_then(|d| d.variants.as_ref())
            .map(|vs| vs.iter().any(|v| v.product_id == pid))
            .unwrap_or(false)
    })
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

// ── Per-device task ───────────────────────────────────────────────────────────

/// Initialise and run the event loop for one device.  Registers the device in
/// `app_state`, emits hotplug signals, and forwards `EmitEvent`s to the D-Bus
/// state map.
async fn run_device(
    info: DeviceInfo,
    config: Arc<DeviceConfig>,
    helper_sock: PathBuf,
    app_state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
) {
    let path_str = info.hidraw_path.to_string_lossy().to_string();
    info!("starting session for {path_str}");

    let fd = match hidraw_client::request_fd(&helper_sock, &path_str).await {
        Ok(fd) => fd,
        Err(e) => {
            error!("failed to get fd for {path_str}: {e}");
            return;
        }
    };

    // Build friendly name from the variant list.
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

    // Create the command channel (D-Bus → device task).
    let (cmd_tx, cmd_rx) = mpsc::channel::<DeviceCommand>(16);

    // Register the device in shared state.
    {
        let mut state = app_state.lock().await;
        state.devices.insert(
            info.hidraw_path.clone(),
            DeviceEntry {
                config: Arc::clone(&config),
                pid: info.pid,
                name: friendly_name.clone(),
                capabilities: capabilities.clone(),
                status: HashMap::new(),
                cmd_tx,
            },
        );
    }

    // Notify listeners that a new device is available.
    let _ = signal_tx.send(SignalEvent::DeviceConnected {
        pid: info.pid,
        name: friendly_name,
        capabilities,
    });

    let (event_tx, mut event_rx) = mpsc::channel::<EmitEvent>(64);

    // Forward EmitEvents: update the state map and emit StatusChanged signal.
    let state_for_events = Arc::clone(&app_state);
    let signal_tx_clone = signal_tx.clone();
    let hidraw_path_clone = info.hidraw_path.clone();
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            info!(signal = %ev.signal, fields = ?ev.fields, "sync event");
            {
                let mut s = state_for_events.lock().await;
                if let Some(entry) = s.devices.get_mut(&hidraw_path_clone) {
                    for (field, val) in &ev.fields {
                        entry
                            .status
                            .insert(field.clone(), state::event_value_to_json(val));
                    }
                }
            }
            let _ = signal_tx_clone.send(SignalEvent::StatusChanged);
        }
    });

    let mut session = DeviceSession::new((*config).clone(), fd);

    match session.device_init().await {
        Ok(events) => {
            info!("{} init events emitted for {path_str}", events.len());
        }
        Err(e) => {
            error!("device init failed for {path_str}: {e}");
            cleanup_device(&app_state, &info.hidraw_path, info.pid, &signal_tx).await;
            return;
        }
    }

    if let Err(e) = session.run_event_loop_with_commands(event_tx, cmd_rx).await {
        match e {
            EngineError::Io(ref io_e)
                if io_e.kind() == std::io::ErrorKind::UnexpectedEof
                    || io_e.kind() == std::io::ErrorKind::BrokenPipe =>
            {
                info!("device {path_str} disconnected");
            }
            _ => error!("event loop error for {path_str}: {e}"),
        }
    }

    cleanup_device(&app_state, &info.hidraw_path, info.pid, &signal_tx).await;
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

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("lam-daemon {}", env!("CARGO_PKG_VERSION"));

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

    let app_state = Arc::new(Mutex::new(AppState {
        configs: configs.clone(),
        devices: HashMap::new(),
        config_dirs: cfg_dirs,
    }));

    let (signal_tx, signal_rx) = broadcast::channel::<SignalEvent>(64);

    // Start D-Bus service.  On failure, continue without it (headless / test env).
    let _dbus_conn = match dbus::start_dbus_service(Arc::clone(&app_state), signal_rx).await {
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
            "device at startup: {} (VID={:#06x} PID={:#06x})",
            dev.hidraw_path.display(),
            dev.vid,
            dev.pid
        );
        if let Some(cfg) = find_config(&configs, dev.pid) {
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
                        if let Some(cfg) = find_config(&configs, dev.pid) {
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
                    }
                }
            }
        } => {}
    }
}
