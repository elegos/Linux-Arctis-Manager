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
    /// Candidate sources set by each feature; `resolve()` picks the
    /// highest-priority one (VC > NC > teardown) independently of the order
    /// NC/VC settings happen to be applied in.
    nc_source: Option<String>,
    vc_source: Option<String>,
}

impl MicRouterState {
    pub fn new() -> Self {
        Self::default()
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Set (or clear, with `None`) NC's candidate output source, then re-resolve
/// priority. Returns `true` on success.
pub async fn set_nc_source(state: &mut MicRouterState, source: Option<String>) -> bool {
    state.nc_source = source;
    resolve(state).await
}

/// Set (or clear, with `None`) VC's candidate output source, then re-resolve
/// priority. Returns `true` on success.
pub async fn set_vc_source(state: &mut MicRouterState, source: Option<String>) -> bool {
    state.vc_source = source;
    resolve(state).await
}

/// VC takes priority over NC; if neither has a candidate source, tear down
/// (no virtual mic at all — apps fall back to whatever they had selected).
async fn resolve(state: &mut MicRouterState) -> bool {
    match resolve_master(&state.vc_source, &state.nc_source) {
        Some(master) => update(state, &master).await,
        None => {
            unload(state).await;
            true
        }
    }
}

/// Pure priority pick: VC > NC > none. Factored out of `resolve()` so the
/// precedence rule is directly testable without a running PipeWire.
fn resolve_master(vc_source: &Option<String>, nc_source: &Option<String>) -> Option<String> {
    vc_source.clone().or_else(|| nc_source.clone())
}

/// Point `Arctis_Manager_Mic` at `master`.
/// Reloads the module only when `master` changes or the module is absent.
/// Returns `true` on success.
async fn update(state: &mut MicRouterState, master: &str) -> bool {
    if state.current_master.as_deref() == Some(master) && state.module_id.is_some() {
        return true;
    }
    unload(state).await;
    load(state, master).await
}

/// Unconditionally unload `Arctis_Manager_Mic` and forget both candidate
/// sources. Used at daemon shutdown; feature code should use
/// `set_nc_source`/`set_vc_source` with `None` instead, so the other
/// feature's source (if any) still gets applied.
pub async fn teardown(state: &mut MicRouterState) {
    state.nc_source = None;
    state.vc_source = None;
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

    // ── resolve_master priority (pure) ──────────────────────────────────

    #[test]
    fn resolve_master_vc_wins_over_nc() {
        assert_eq!(
            resolve_master(&Some("vc_mic".to_owned()), &Some("nc_mic".to_owned())),
            Some("vc_mic".to_owned())
        );
    }

    #[test]
    fn resolve_master_falls_back_to_nc_when_vc_absent() {
        assert_eq!(
            resolve_master(&None, &Some("nc_mic".to_owned())),
            Some("nc_mic".to_owned())
        );
    }

    #[test]
    fn resolve_master_none_when_both_absent() {
        assert_eq!(resolve_master(&None, &None), None);
    }

    #[test]
    fn mic_name_constant() {
        assert_eq!(MIC_NAME, "Arctis_Manager_Mic");
    }
}
