// Manages `Arctis_Manager_Mic` — the single stable PulseAudio/PipeWire source
// that apps select once and always record from.
//
// Priority: VC output > NC output > teardown (no virtual mic).
//
// `update()` is idempotent: calling it with the same master twice is a no-op.

use tracing::{info, warn};

use crate::audio;

/// Stable user-visible source name.
pub const MIC_NAME: &str = "Arctis_Manager_Mic";
pub const MIC_DESC: &str = "Arctis Manager Mic";

// ── Runtime ───────────────────────────────────────────────────────────────────

/// Loaded module index for `Arctis_Manager_Mic`.
#[derive(Debug, Default)]
pub struct MicRouterState {
    module_id: Option<u32>,
    current_master: Option<String>,
}

impl MicRouterState {
    pub fn new() -> Self {
        Self::default()
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Point `Arctis_Manager_Mic` at `master`.
/// Reloads the module only when `master` changes or the module is absent.
/// Returns `true` on success.
pub async fn update(state: &mut MicRouterState, master: &str) -> bool {
    if state.current_master.as_deref() == Some(master) && state.module_id.is_some() {
        return true;
    }
    unload(state).await;
    load(state, master).await
}

/// Unload `Arctis_Manager_Mic` if loaded.
pub async fn teardown(state: &mut MicRouterState) {
    unload(state).await;
}

// ── Internal helpers ──────────────────────────────────────────────────────────

async fn load(state: &mut MicRouterState, master: &str) -> bool {
    // Check whether the source already exists (leftover from a previous run).
    if let Some(existing_id) = find_existing_module().await {
        state.module_id = Some(existing_id);
        state.current_master = Some(master.to_owned());
        info!("mic_router: reusing existing {MIC_NAME} (module {existing_id})");
        return true;
    }

    // node.virtual=false + device.class=sound makes the source appear in
    // KDE/GNOME input device lists (module-remap-source sets device.class=filter
    // by default, which hides it).
    let props = format!(
        "node.virtual=false \
         node.description=\\\"{MIC_DESC}\\\" \
         device.description=\\\"{MIC_DESC}\\\" \
         device.class=sound"
    );
    let args = format!("source_name={MIC_NAME} master={master} source_properties=\"{props}\"");

    for module in &["module-remap-source", "module-virtual-source"] {
        match audio::load_module_pub(module, &args).await {
            Some(id) => {
                state.module_id = Some(id);
                state.current_master = Some(master.to_owned());
                info!("mic_router: {MIC_NAME} → {master} (module {id}, via {module})");
                // Belt-and-suspenders: set description via pactl in case PipeWire
                // ignored the source_properties description above.
                let _ = tokio::process::Command::new("pactl")
                    .args([
                        "set-source-properties",
                        MIC_NAME,
                        &format!("node.description={MIC_DESC}"),
                    ])
                    .output()
                    .await;
                return true;
            }
            None => {
                warn!("mic_router: {module} failed");
            }
        }
    }
    warn!("mic_router: could not create {MIC_NAME} → {master}");
    false
}

async fn unload(state: &mut MicRouterState) {
    if let Some(id) = state.module_id.take() {
        if let Err(e) = audio::unload_module_by_id(id).await {
            warn!("mic_router: unload module {id} failed: {e}");
        } else {
            info!("mic_router: unloaded module {id}");
        }
    }
    state.current_master = None;
}

/// Return the `owner_module` index of an existing `Arctis_Manager_Mic` source, or `None`.
/// Prevents duplicate modules when the daemon restarts without a clean teardown.
async fn find_existing_module() -> Option<u32> {
    let out = tokio::process::Command::new("pactl")
        .args(["-f", "json", "list", "sources"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())?;

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    for source in json.as_array()? {
        let name = source["properties"]["node.name"]
            .as_str()
            .or_else(|| source["name"].as_str())?;
        if name == MIC_NAME {
            // `owner_module` is a string in pactl JSON output.
            let mod_str = source["owner_module"].as_str()?;
            return mod_str.parse().ok();
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_inactive() {
        let s = MicRouterState::new();
        assert!(s.module_id.is_none());
        assert!(s.current_master.is_none());
    }

    #[test]
    fn mic_name_constant() {
        assert_eq!(MIC_NAME, "Arctis_Manager_Mic");
    }
}
