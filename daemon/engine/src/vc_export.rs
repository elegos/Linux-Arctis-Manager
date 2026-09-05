// One-shot `.pth` -> `.onnx` export orchestration.
//
// `export_onnx.py` (the "one Python piece that stays", see
// `docs/voice-changing-feature.md`) is a real dependency-acquisition case:
// it needs `torch`/`numpy`/`onnx`, none of which are part of this app's own
// GUI venv (CPU-only, one-shot — deliberately kept out of both the daemon's
// Rust dependency tree and the GUI's Python venv). Per
// `docs/v3-backlog.md`'s [E10-S6a] dependency-acquisition philosophy: prefer
// a distro-packaged `python3-torch` equivalent if one is already
// importable, else offer to create a small per-user pip venv — but only
// ever with the user's explicit consent to the exact command, never
// installed silently. `dbus.rs`'s `EnsureModelExported` is the entry point
// that ties this together with an actual export run, triggered right after
// a model download completes or the first time an unexported model is
// selected in the GUI.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

/// `~/.local/share/arctis_manager/ai_env` — same location (and purpose) as
/// the legacy Python daemon's `ai_deps.py` venv, but far slimmer: only
/// `torch`/`numpy`/`onnx` (CPU wheels), never `torchaudio`/`faiss-cpu` —
/// export is a one-shot offline conversion, not a live inference backend.
pub fn ai_env_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join("arctis_manager")
        .join("ai_env")
}

fn ai_env_python(ai_env: &Path) -> PathBuf {
    ai_env.join("bin").join("python")
}

pub const EXPORT_DEPS_PACKAGES: &[&str] = &["torch", "numpy", "onnx"];
/// Export never touches a GPU, so always pull the small CPU wheel rather
/// than PyPI's default (CUDA-bundled, ~10x larger) `torch` build.
pub const TORCH_CPU_INDEX_URL: &str = "https://download.pytorch.org/whl/cpu";

/// The daemon binary's own install layout (`Makefile`: `BINDIR = PREFIX/bin`,
/// the Python package lives under `PREFIX/lib/linux-arctis-manager/venv`) —
/// resolved relative to the *running* binary rather than a hardcoded
/// `/usr/...` prefix, so it survives `PREFIX` overrides
/// (`make PREFIX=/usr/local install`, packaging into `/opt`, etc).
fn main_venv_dir(bin_dir: &Path) -> PathBuf {
    bin_dir.join("../lib/linux-arctis-manager/venv")
}

/// `<venv>/lib/python3.*/site-packages` — the one path this module ever
/// needs on `PYTHONPATH`, so a *different* interpreter (system `python3`,
/// or the slim `ai_env` above) can `import linux_arctis_manager...` without
/// that package being installed into it. Picks the highest version present
/// if more than one somehow is.
fn find_site_packages(venv_dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(venv_dir.join("lib"))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("python3."))
        })
        .collect();
    entries.sort();
    entries.into_iter().rev().find_map(|p| {
        p.join("site-packages")
            .is_dir()
            .then(|| p.join("site-packages"))
    })
}

/// `None` when the daemon isn't running from an installed layout (e.g. a
/// dev `cargo run` build) — export needs `main.py`'s actual install to find
/// the `linux_arctis_manager` package.
pub fn main_site_packages() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?; // nosemgrep: rust.lang.security.current-exe.current-exe — path lookup for locating the install layout, not a security decision
    let bin_dir = exe.parent()?;
    find_site_packages(&main_venv_dir(bin_dir))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportDepsStatus {
    pub available: bool,
    /// `None` when `available` is `false`.
    pub python: Option<PathBuf>,
    /// Human-readable command for consent display — also exactly what
    /// [`install_export_deps`] runs.
    pub install_command: String,
}

fn install_command_hint(ai_env: &Path) -> String {
    // `--extra-index-url`, not `--index-url`: the PyTorch CPU index only
    // hosts the pytorch-ecosystem wheels, so replacing PyPI outright with
    // `--index-url` would make pip unable to find `numpy`/`onnx` at all.
    format!(
        "python3 -m venv {} && {}/bin/pip install {} --extra-index-url {}",
        ai_env.display(),
        ai_env.display(),
        EXPORT_DEPS_PACKAGES.join(" "),
        TORCH_CPU_INDEX_URL,
    )
}

async fn python_has_deps(python: &Path) -> bool {
    Command::new(python)
        .args(["-c", "import torch, numpy, onnx"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Checks, in preference order: a system `python3` already on `PATH` with
/// `torch`/`numpy`/`onnx` importable (a distro package, or a user's own
/// setup), then the slim `ai_env` venv this module manages. Never installs
/// anything itself — see [`install_export_deps`] for the consent-gated
/// install path.
pub async fn detect_export_deps() -> ExportDepsStatus {
    let ai_env = ai_env_dir();
    let install_command = install_command_hint(&ai_env);

    if python_has_deps(Path::new("python3")).await {
        return ExportDepsStatus {
            available: true,
            python: Some(PathBuf::from("python3")),
            install_command,
        };
    }
    let ai_python = ai_env_python(&ai_env);
    if ai_python.is_file() && python_has_deps(&ai_python).await {
        return ExportDepsStatus {
            available: true,
            python: Some(ai_python),
            install_command,
        };
    }
    ExportDepsStatus {
        available: false,
        python: None,
        install_command,
    }
}

/// Creates the `ai_env` venv (if missing) and installs
/// [`EXPORT_DEPS_PACKAGES`] into it via the CPU wheel index — the exact
/// command [`detect_export_deps`]'s `install_command` shows for consent.
/// Only ever called after the GUI has shown that command and the user
/// agreed to run it.
pub async fn install_export_deps() -> Result<(), String> {
    let ai_env = ai_env_dir();

    let venv_status = Command::new("python3")
        .args(["-m", "venv"])
        .arg(&ai_env)
        .status()
        .await
        .map_err(|e| format!("failed to run 'python3 -m venv': {e}"))?;
    if !venv_status.success() {
        return Err("python3 -m venv failed".to_owned());
    }

    let pip = ai_env.join("bin").join("pip");
    let output = Command::new(&pip)
        .arg("install")
        .args(EXPORT_DEPS_PACKAGES)
        .args(["--extra-index-url", TORCH_CPU_INDEX_URL])
        .output()
        .await
        .map_err(|e| format!("failed to run pip install: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "pip install failed: {}",
            stderr.lines().last().unwrap_or("unknown error")
        ));
    }
    Ok(())
}

/// Runs `export_onnx.py` on `pth_path`, writing `<stem>.onnx` next to it.
/// Fails clearly (rather than guessing) when no interpreter has the
/// dependencies, or the installed `linux_arctis_manager` package can't be
/// located — call [`detect_export_deps`] first to distinguish those from a
/// real export failure before presenting an error to the user.
pub async fn run_export(pth_path: &Path) -> Result<PathBuf, String> {
    let deps = detect_export_deps().await;
    let Some(python) = deps.python else {
        return Err("export dependencies (torch/numpy/onnx) are not installed".to_owned());
    };
    let Some(site_packages) = main_site_packages() else {
        return Err("could not locate the installed linux_arctis_manager package".to_owned());
    };

    let output = Command::new(&python)
        .env("PYTHONPATH", &site_packages)
        .args(["-m", "linux_arctis_manager.voice_changer.rvc.export_onnx"])
        .arg(pth_path)
        .output()
        .await
        .map_err(|e| format!("failed to run export_onnx.py: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "export_onnx.py failed: {}",
            stderr.lines().last().unwrap_or("unknown error")
        ));
    }

    let onnx_path = pth_path.with_extension("onnx");
    if !onnx_path.is_file() {
        return Err("export_onnx.py reported success but wrote no .onnx file".to_owned());
    }
    Ok(onnx_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn find_site_packages_none_when_venv_dir_missing() {
        let base = tempdir().unwrap();
        assert!(find_site_packages(&base.path().join("no-such-venv")).is_none());
    }

    #[test]
    fn find_site_packages_finds_the_python_dir() {
        let base = tempdir().unwrap();
        let venv = base.path().join("venv");
        std::fs::create_dir_all(venv.join("lib/python3.12/site-packages")).unwrap();
        let found = find_site_packages(&venv).unwrap();
        assert_eq!(found, venv.join("lib/python3.12/site-packages"));
    }

    #[test]
    fn find_site_packages_picks_the_highest_version_when_several_present() {
        let base = tempdir().unwrap();
        let venv = base.path().join("venv");
        std::fs::create_dir_all(venv.join("lib/python3.10/site-packages")).unwrap();
        std::fs::create_dir_all(venv.join("lib/python3.12/site-packages")).unwrap();
        std::fs::create_dir_all(venv.join("lib/python3.11/site-packages")).unwrap();
        let found = find_site_packages(&venv).unwrap();
        assert_eq!(found, venv.join("lib/python3.12/site-packages"));
    }

    #[test]
    fn find_site_packages_ignores_non_python_dirs() {
        let base = tempdir().unwrap();
        let venv = base.path().join("venv");
        std::fs::create_dir_all(venv.join("lib/bin")).unwrap();
        assert!(find_site_packages(&venv).is_none());
    }

    #[test]
    fn main_venv_dir_matches_the_makefile_layout() {
        let bin_dir = Path::new("/usr/local/bin");
        assert_eq!(
            main_venv_dir(bin_dir),
            Path::new("/usr/local/bin/../lib/linux-arctis-manager/venv")
        );
    }

    #[test]
    fn install_command_hint_mentions_the_venv_path_and_cpu_index() {
        let hint = install_command_hint(Path::new("/home/u/.local/share/arctis_manager/ai_env"));
        assert!(hint.contains("/home/u/.local/share/arctis_manager/ai_env"));
        assert!(hint.contains("torch"));
        assert!(hint.contains("numpy"));
        assert!(hint.contains("onnx"));
        assert!(hint.contains(TORCH_CPU_INDEX_URL));
    }

    #[tokio::test]
    async fn detect_export_deps_false_when_nothing_available() {
        // A bogus HOME with no ai_env, and python3 on this CI/dev machine
        // may or may not have torch/numpy/onnx — so this only asserts the
        // shape/consistency of the result, not a specific `available` value
        // (that would depend on the machine's own Python setup).
        let status = detect_export_deps().await;
        assert_eq!(status.available, status.python.is_some());
        assert!(status.install_command.contains("torch"));
    }

    // ── live: real venv creation + real pip install ──────────────────────
    // Not run by default — hits the real network (downloads real CPU
    // torch/numpy/onnx wheels, a few hundred MB) and takes a minute or
    // more. Verifies the full needs_deps -> consent -> install -> available
    // cycle end to end *without* touching the real system `python3` or the
    // real `ai_env` (which may well already satisfy the check on a dev
    // machine, same as it does on this project's own — see the session
    // that motivated this test): `PATH` is shadowed with a shim `python3`
    // that fails only the `import torch, numpy, onnx` probe (so
    // `detect_export_deps`'s system-python3 check reports unavailable) and
    // delegates every other invocation (`-m venv`) to the real interpreter,
    // and `XDG_DATA_HOME` points at a fresh temp dir so `ai_env_dir()`
    // starts out empty.
    //
    // Run manually with:
    // `cargo test --bin lam-daemon -- --ignored live_install_export_deps_end_to_end --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_install_export_deps_end_to_end() {
        use std::os::unix::fs::PermissionsExt;

        let real_python3 = String::from_utf8(
            std::process::Command::new("sh")
                .args(["-c", "command -v python3"])
                .output()
                .expect("run `command -v python3`")
                .stdout,
        )
        .expect("utf8")
        .trim()
        .to_owned();
        assert!(!real_python3.is_empty(), "no real python3 found on PATH");

        let tmp = tempdir().unwrap();
        let shim_dir = tmp.path().join("shim-bin");
        std::fs::create_dir_all(&shim_dir).unwrap();
        let shim_path = shim_dir.join("python3");
        std::fs::write(
            &shim_path,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"-c\" ] && echo \"$2\" | grep -q 'import torch'; then\n\
                 \x20\x20exit 1\n\
                 fi\n\
                 exec {real_python3} \"$@\"\n"
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&shim_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim_path, perms).unwrap();

        let old_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: this test is `#[ignore]`d and meant to be run alone
        // (`--ignored <name>`), never concurrently with other tests that
        // read PATH/XDG_DATA_HOME — same caveat as any other env-var-based
        // live test in this codebase (e.g. LAM_ORT_DYLIB_PATH elsewhere).
        // Both vars stay set for the whole before/install/after sequence —
        // `ai_env_dir()` must resolve to the *same* temp location the whole
        // way through, and the shim must keep "hiding" system deps for the
        // `after` check to actually prove the ai_env (not PATH) is what's
        // now satisfying it.
        unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            std::env::set_var("PATH", format!("{}:{old_path}", shim_dir.display()));
            std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        }

        let before = detect_export_deps().await;
        assert!(
            !before.available,
            "expected deps unavailable under the shimmed PATH, got {before:?}"
        );
        assert!(before.python.is_none());

        let result = install_export_deps().await;
        let after = if result.is_ok() {
            Some(detect_export_deps().await)
        } else {
            None
        };

        unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            std::env::set_var("PATH", &old_path);
            std::env::remove_var("XDG_DATA_HOME");
        }

        result.expect("install_export_deps should succeed against a real pip/network");
        let after = after.expect("unreachable: result was Ok");
        assert!(
            after.available,
            "expected deps available after install, got {after:?}"
        );
        assert_eq!(
            after.python,
            Some(ai_env_python(&ai_env_dir_for(&tmp.path().join("data")))),
            "expected the ai_env (not the shimmed system python3) to satisfy the check"
        );
        eprintln!("resolved python after install: {:?}", after.python);
    }

    /// Test-only: `ai_env_dir()` reads `XDG_DATA_HOME` at call time, but
    /// this assertion runs *after* that var has already been restored —
    /// so it rebuilds the same path directly instead.
    fn ai_env_dir_for(xdg_data_home: &Path) -> PathBuf {
        xdg_data_home.join("arctis_manager").join("ai_env")
    }
}
