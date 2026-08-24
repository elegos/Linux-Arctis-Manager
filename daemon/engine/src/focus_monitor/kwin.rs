// KWin Wayland focus backend (KWin 6 scripting API).
//
// KWin 6 removed activeWindow() and windowActivated D-Bus signal from /KWin.
// Instead we inject a KWin JS script that subscribes to workspace.windowActivated
// and calls back our daemon via a temporary D-Bus interface.  This works for both
// Wayland-native (Firefox) and XWayland (Steam) windows.

use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use zbus::{interface, proxy, Connection};

use super::event::FocusEvent;

// ── KWin Scripting proxy ──────────────────────────────────────────────────────

#[proxy(
    interface = "org.kde.kwin.Scripting",
    default_service = "org.kde.KWin",
    default_path = "/Scripting"
)]
trait KWinScripting {
    #[zbus(name = "loadScript")]
    fn load_script(&self, file_path: &str, plugin_name: &str) -> zbus::Result<i32>;

    #[zbus(name = "unloadScript")]
    fn unload_script(&self, plugin_name: &str) -> zbus::Result<bool>;

    #[zbus(name = "isScriptLoaded")]
    fn is_script_loaded(&self, plugin_name: &str) -> zbus::Result<bool>;
}

// ── Callback D-Bus interface (receives calls from the KWin script) ────────────

const CALLBACK_IFACE: &str = "name.giacomofurlan.ArctisManager.FocusCallback";
const CALLBACK_PATH: &str = "/FocusCallback";
const PLUGIN_NAME: &str = "lam-focus-monitor";

struct FocusCallbackIface {
    tx: Arc<Mutex<mpsc::Sender<(String, u32)>>>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.FocusCallback")]
impl FocusCallbackIface {
    async fn on_window_focused(&self, app_id: String, pid: u32) {
        tracing::info!("focus/kwin: callback appId={app_id:?} pid={pid}");
        let _ = self.tx.lock().await.send((app_id, pid)).await;
    }
}

// ── Run ───────────────────────────────────────────────────────────────────────

pub async fn run(ev_tx: mpsc::Sender<FocusEvent>) {
    // Own connection for the callback D-Bus object (distinct from daemon's bus).
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("focus/kwin: D-Bus session connection failed: {e}");
            return;
        }
    };

    let unique_name = match conn.unique_name() {
        Some(n) => n.to_string(),
        None => {
            tracing::warn!("focus/kwin: could not obtain unique D-Bus name");
            return;
        }
    };

    let (cb_tx, mut cb_rx) = mpsc::channel::<(String, u32)>(32);
    let iface = FocusCallbackIface {
        tx: Arc::new(Mutex::new(cb_tx)),
    };

    if let Err(e) = conn.object_server().at(CALLBACK_PATH, iface).await {
        tracing::warn!("focus/kwin: failed to register callback interface: {e}");
        return;
    }

    // Write the KWin script, substituting our unique bus name.
    let script_path = match write_script(&unique_name) {
        Some(p) => p,
        None => {
            tracing::warn!("focus/kwin: could not write KWin script file");
            return;
        }
    };

    // Connect to KWin scripting and load the script.
    let scripting = match KWinScriptingProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("focus/kwin: KWin scripting proxy failed: {e}");
            return;
        }
    };

    // Unload any leftover script from a previous daemon run.
    let _ = scripting.unload_script(PLUGIN_NAME).await;

    match scripting.load_script(&script_path, PLUGIN_NAME).await {
        Ok(id) if id >= 0 => {
            tracing::info!("focus/kwin: KWin script loaded (id={id}), listening for focus events");
        }
        Ok(id) => {
            tracing::warn!("focus/kwin: KWin loadScript returned {id} (error)");
            return;
        }
        Err(e) => {
            tracing::warn!("focus/kwin: KWin loadScript failed: {e}");
            return;
        }
    }

    // Drain focus events from the KWin script callbacks.
    while let Some((app_id, pid)) = cb_rx.recv().await {
        let class = if app_id.is_empty() { None } else { Some(app_id) };
        let event = FocusEvent::Focused {
            pid: if pid == 0 { None } else { Some(pid) },
            class,
        };
        if ev_tx.send(event).await.is_err() {
            break;
        }
    }

    let _ = scripting.unload_script(PLUGIN_NAME).await;
    let _ = std::fs::remove_file(&script_path);
}

// ── Script file ───────────────────────────────────────────────────────────────

fn write_script(bus_name: &str) -> Option<String> {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_string());
    let path = format!("{dir}/lam-kwin-focus.js");

    // In KWin 6, callDBus() is workspace.callDBus(), not a global function.
    // console.log() output goes to KWin's journal for diagnostics.
    let content = format!(
        r#"(function() {{
    var svc  = "{bus_name}";
    var path = "{path}";
    var iface = "{iface}";

    function report(window) {{
        if (!window) {{
            console.log("[lam-focus] windowActivated: null");
            return;
        }}
        var appId = window.desktopFileName || window.resourceClass || "";
        var pid   = window.pid || 0;
        console.log("[lam-focus] windowActivated: appId=" + appId + " pid=" + pid);
        KWin.callDBus(svc, path, iface, "OnWindowFocused", appId, pid);
    }}

    workspace.windowActivated.connect(report);

    // Unconditional test call: verifies KWin.callDBus() works at load time.
    KWin.callDBus(svc, path, iface, "OnWindowFocused", "_script_loaded_", 0);

    var active = workspace.activeWindow;
    if (active) report(active);
}})();
"#,
        bus_name = bus_name,
        path = CALLBACK_PATH,
        iface = CALLBACK_IFACE,
    );

    std::fs::write(&path, content).ok()?;
    Some(path)
}
