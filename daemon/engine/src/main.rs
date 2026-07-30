mod device_session;
mod engine_error;
mod hidraw_client;
mod hotplug;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use device_config::DeviceConfig;
use device_session::DeviceSession;
use engine_error::EngineError;
use hotplug::DeviceInfo;

// ── Config loading ────────────────────────────────────────────────────────────

/// Scan `dir` for `*.yaml` files and return successfully parsed configs.
fn load_configs(dir: &Path) -> Vec<Arc<DeviceConfig>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let search_dirs = [dir];
    let mut configs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        match device_config::load(&path, &search_dirs) {
            Ok(cfg) => configs.push(Arc::new(cfg)),
            Err(e) => warn!("skipping {}: {e}", path.display()),
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
fn helper_sock_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("lam-hidraw-helper.sock")
}

/// Return the directory where device YAML configs are stored.
fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("arctis_manager/devices")
}

// ── Per-device task ───────────────────────────────────────────────────────────

/// Initialise and run the event loop for one device.  Logs errors internally
/// so the task always returns `()`.
async fn run_device(info: DeviceInfo, config: Arc<DeviceConfig>, helper_sock: PathBuf) {
    let path_str = info.hidraw_path.to_string_lossy();
    info!("starting session for {path_str}");

    let fd = match hidraw_client::request_fd(&helper_sock, &path_str).await {
        Ok(fd) => fd,
        Err(e) => {
            error!("failed to get fd for {path_str}: {e}");
            return;
        }
    };

    let (event_tx, mut event_rx) = mpsc::channel::<device_config::sync_dispatcher::EmitEvent>(64);

    // Log emitted sync events (D-Bus forwarding comes in E4).
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            info!(signal = %ev.signal, fields = ?ev.fields, "sync event");
        }
    });

    let mut session = DeviceSession::new((*config).clone(), fd);

    match session.device_init().await {
        Ok(events) => {
            info!("{} init events emitted for {path_str}", events.len());
            for ev in events {
                info!(signal = %ev.signal, "init event: {}", ev.signal);
            }
        }
        Err(e) => {
            error!("device init failed for {path_str}: {e}");
            return;
        }
    }

    if let Err(e) = session.run_event_loop(event_tx).await {
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
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("lam-daemon {}", env!("CARGO_PKG_VERSION"));

    let cfg_dir = config_dir();
    let configs = load_configs(&cfg_dir);
    if configs.is_empty() {
        warn!("no device configs found in {}", cfg_dir.display());
    } else {
        info!("loaded {} device config(s)", configs.len());
    }

    let helper_sock = helper_sock_path();

    // Enumerate devices already connected at startup.
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
        match find_config(&configs, dev.pid) {
            Some(cfg) => {
                let cfg = Arc::clone(cfg);
                let sock = helper_sock.clone();
                let path = dev.hidraw_path.clone();
                let handle = tokio::spawn(run_device(dev, cfg, sock));
                tasks.insert(path, handle);
            }
            None => info!("no config for PID {:#06x}, skipping", dev.pid),
        }
    }

    // Watch for hotplug events.
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
                        match find_config(&configs, dev.pid) {
                            Some(cfg) => {
                                let cfg = Arc::clone(cfg);
                                let sock = helper_sock.clone();
                                let path = dev.hidraw_path.clone();
                                let handle = tokio::spawn(run_device(dev, cfg, sock));
                                tasks.insert(path, handle);
                            }
                            None => info!("no config for PID {:#06x}", dev.pid),
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
