//! Optional, WirePlumber-backed hiding of the physical Arctis sink while the
//! virtual `Arctis_Media`/`Arctis_Chat` sinks are active (GH #67).
//!
//! WirePlumber enforces per-client object permissions via `client:update_permissions{}`
//! calls that must be issued by a script running *inside* the WirePlumber process,
//! reacting to every client as it connects — there is no external one-shot call
//! that can hide an object from clients that connect *after* the call runs. So
//! the actual hiding logic lives in a permanently-installed WirePlumber Lua
//! policy script (`packaging/wireplumber/sink-visibility.lua`, registered as an
//! always-loaded component via `51-lam-sink-visibility.conf`), not here.
//!
//! This module only owns the daemon's side of the handshake: a lease file whose
//! mere *presence* means "keep the physical sink hidden" (the script watches it
//! via WirePlumber's native `file-monitor-api` plugin, not by polling). Crash
//! safety — never leaving the sink hidden forever if the daemon dies — is
//! deliberately NOT this module's job: `lam-daemon.service`'s own
//! `ExecStopPost=` removes the lease file unconditionally whenever the unit
//! stops, including on crash/SIGKILL, so this module only has to handle the
//! orderly-shutdown path.

use std::path::PathBuf;

/// Path to the lease file the WirePlumber policy script watches.
pub fn lease_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("lam-sink-visibility.lease")
}

/// Best-effort detection of whether WirePlumber is usable on this system.
/// Kept synchronous (like the other runtime-capability checks in
/// `vc_onnxruntime_detect.rs`) since it's a single quick local command,
/// not worth threading an `.await` through the many `settings_config_json`
/// call sites for.
pub fn wireplumber_available() -> bool {
    std::process::Command::new("wpctl")
        .arg("status")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Holds the physical sink hidden for as long as it's alive: its `start()`
/// creates the lease file, its `stop()` removes it. `sink_name` isn't
/// written into the lease (the WirePlumber script re-derives the physical
/// sink itself, the same way `audio::find_physical_sink` does) — it's only
/// for the warning log below, so a caller doesn't need a second lookup just
/// to see which sink a failed lease write was about.
pub struct LeaseGuard;

impl LeaseGuard {
    pub fn start(sink_name: String) -> Self {
        let path = lease_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, b"") {
            tracing::warn!("failed to create sink-visibility lease for {sink_name}: {e}");
        }
        Self
    }

    /// Deletes the lease file immediately, restoring the physical sink's
    /// visibility right away.
    pub async fn stop(self) {
        let _ = tokio::fs::remove_file(lease_path()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_path_respects_xdg_runtime_dir_and_falls_back_to_tmp() {
        // Both assertions live in one test (rather than two `set_var`-mutating
        // tests) since env vars are process-global and Rust runs tests on
        // multiple threads by default — splitting them risks one test's
        // `remove_var` racing the other's `set_var`.
        let original = std::env::var_os("XDG_RUNTIME_DIR");

        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/lam-test-runtime");
        assert_eq!(
            lease_path(),
            PathBuf::from("/tmp/lam-test-runtime/lam-sink-visibility.lease")
        );

        std::env::remove_var("XDG_RUNTIME_DIR");
        assert_eq!(
            lease_path(),
            PathBuf::from("/tmp/lam-sink-visibility.lease")
        );

        match original {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    #[tokio::test]
    async fn start_then_stop_creates_then_removes_lease_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

        let guard = LeaseGuard::start("Test_Sink".to_string());
        assert!(lease_path().exists());

        guard.stop().await;
        assert!(!lease_path().exists());

        std::env::remove_var("XDG_RUNTIME_DIR");
    }
}
