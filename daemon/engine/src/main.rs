mod hotplug;

use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("lam-daemon {}", env!("CARGO_PKG_VERSION"));

    // Enumerate devices already connected at startup (E1-S4 will init them).
    match hotplug::scan_existing(&[]) {
        Ok(devs) => {
            for d in &devs {
                info!(
                    "device at startup: {} (VID={:#06x} PID={:#06x})",
                    d.hidraw_path.display(),
                    d.vid,
                    d.pid
                );
            }
        }
        Err(e) => warn!("udev enumeration failed: {e}"),
    }

    // Run the udev watch loop and the event dispatch loop concurrently.
    // AsyncMonitorSocket is not Send, so both run on this thread via select!.
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
                    hotplug::HotplugEvent::Added(d) => {
                        info!(
                            "hotplug add: {} (VID={:#06x} PID={:#06x})",
                            d.hidraw_path.display(),
                            d.vid,
                            d.pid
                        );
                    }
                    hotplug::HotplugEvent::Removed(d) => {
                        info!(
                            "hotplug remove: {} (VID={:#06x} PID={:#06x})",
                            d.hidraw_path.display(),
                            d.vid,
                            d.pid
                        );
                    }
                }
            }
        } => {}
    }
}
