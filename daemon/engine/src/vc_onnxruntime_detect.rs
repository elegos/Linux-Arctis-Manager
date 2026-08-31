// Guided `libonnxruntime` install helper ([E10-S7]): detects the user's
// GPU vendor and Linux distribution/package manager, picks the matching
// install tutorial, and probes common system paths for an already-installed
// `libonnxruntime.so` — the same detection this module does also answers
// [E10-S6a]'s open "where does the daemon find a real onnxruntime .so"
// question for the live RVC audio chain, so it's built once here rather
// than as a throwaway path list in two places.
//
// Design (see docs/v3-backlog.md's [E10-S7] entry for the full account):
// tutorials are plain-text files (not code), each a full mini-guide rather
// than a bare command, so they stay useful after a minor package-manager
// command changes and can explain multi-step cases (enabling a repository,
// a package conflicting with another). The daemon/GUI only ever *shows*
// the chosen tutorial — it never invokes a system package manager itself;
// a `pip install --user` fallback is called out *within* the tutorial text
// as something the GUI may run only after the user's own explicit consent
// to the exact shown command (the same trust model as this assistant
// asking before running a shell command) — see `daemon/Cargo.toml`'s and
// `packaging/{fedora,debian,arch}`'s dependency-acquisition philosophy note.

use std::path::PathBuf;

// ── GPU vendor detection ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

impl GpuVendor {
    /// The tutorial filename suffix for this vendor, or `None` when there's
    /// no vendor-specific tutorial (falls back to the plain CPU one).
    fn tutorial_suffix(self) -> Option<&'static str> {
        match self {
            GpuVendor::Nvidia => Some("nvidia"),
            GpuVendor::Amd => Some("amd"),
            GpuVendor::Intel | GpuVendor::Unknown => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            GpuVendor::Nvidia => "NVIDIA",
            GpuVendor::Amd => "AMD",
            GpuVendor::Intel => "Intel",
            GpuVendor::Unknown => "unknown",
        }
    }
}

/// PCI vendor ID (as found in `/sys/class/drm/card*/device/vendor`, a
/// `0x`-prefixed hex string) -> [`GpuVendor`]. Pure: takes the ID directly
/// rather than reading `/sys` itself, so it's testable without root or a
/// real GPU.
fn classify_vendor_id(id: &str) -> GpuVendor {
    match id.trim().to_ascii_lowercase().as_str() {
        "0x10de" => GpuVendor::Nvidia,
        "0x1002" => GpuVendor::Amd,
        "0x8086" => GpuVendor::Intel,
        _ => GpuVendor::Unknown,
    }
}

/// Priority when multiple GPUs are present (e.g. a laptop with an Intel
/// iGPU and an NVIDIA dGPU): prefer whichever one an accelerated onnxruntime
/// build could actually use.
fn best_vendor(vendors: impl IntoIterator<Item = GpuVendor>) -> GpuVendor {
    let mut found = Vec::new();
    for v in vendors {
        if !found.contains(&v) {
            found.push(v);
        }
    }
    for preferred in [GpuVendor::Nvidia, GpuVendor::Amd, GpuVendor::Intel] {
        if found.contains(&preferred) {
            return preferred;
        }
    }
    GpuVendor::Unknown
}

/// Scans `/sys/class/drm/card*/device/vendor` for PCI vendor IDs — no
/// `lspci`/root needed. Returns the highest-priority vendor found (NVIDIA >
/// AMD > Intel > Unknown), matching [`best_vendor`].
pub fn detect_gpu_vendor() -> GpuVendor {
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let vendor_path = entry.path().join("device/vendor");
            if let Ok(content) = std::fs::read_to_string(&vendor_path) {
                ids.push(classify_vendor_id(&content));
            }
        }
    }
    best_vendor(ids)
}

// ── Distro / package manager detection ───────────────────────────────────

/// Distros this module has a dedicated tutorial set for.
const KNOWN_DISTROS: &[&str] = &["fedora", "arch", "debian", "ubuntu"];

/// Parse `/etc/os-release`'s `ID=` field (pure — takes the file content
/// directly). Handles the optional double-quoting the format allows.
fn parse_os_release_id(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("ID=") {
            return Some(rest.trim().trim_matches('"').to_owned());
        }
    }
    None
}

pub fn detect_distro_id() -> Option<String> {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| parse_os_release_id(&s))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Dnf,
    Apt,
    Pacman,
}

impl PackageManager {
    fn tutorial_prefix(self) -> &'static str {
        match self {
            PackageManager::Dnf => "dnf",
            PackageManager::Apt => "apt",
            PackageManager::Pacman => "pacman",
        }
    }

    /// The binary to look for in `PATH` to detect this package manager.
    fn binary_name(self) -> &'static str {
        match self {
            PackageManager::Dnf => "dnf",
            PackageManager::Apt => "apt",
            PackageManager::Pacman => "pacman",
        }
    }
}

/// Checked in this order — the first one found in `PATH` wins. Order
/// doesn't matter much in practice (a system realistically has exactly one
/// of these), but is fixed for determinism.
const ALL_PACKAGE_MANAGERS: &[PackageManager] = &[
    PackageManager::Dnf,
    PackageManager::Apt,
    PackageManager::Pacman,
];

fn binary_in_path(name: &str, path_var: &str) -> bool {
    std::env::split_paths(path_var).any(|dir| dir.join(name).is_file())
}

pub fn detect_package_manager() -> Option<PackageManager> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    ALL_PACKAGE_MANAGERS
        .iter()
        .copied()
        .find(|pm| binary_in_path(pm.binary_name(), &path_var))
}

// ── Tutorial selection ───────────────────────────────────────────────────

macro_rules! tutorial {
    ($name:literal) => {
        include_str!(concat!(
            "../../../packaging/onnxruntime-install/",
            $name,
            ".txt"
        ))
    };
}

/// `(distro-or-pkgmgr, vendor-suffix-or-"cpu")` -> tutorial text, for every
/// file under `packaging/onnxruntime-install/`.
const TUTORIALS: &[(&str, &str, &str)] = &[
    ("fedora", "cpu", tutorial!("fedora-cpu")),
    ("fedora", "amd", tutorial!("fedora-amd")),
    ("fedora", "nvidia", tutorial!("fedora-nvidia")),
    ("arch", "cpu", tutorial!("arch-cpu")),
    ("arch", "amd", tutorial!("arch-amd")),
    ("arch", "nvidia", tutorial!("arch-nvidia")),
    ("debian", "cpu", tutorial!("debian-cpu")),
    ("debian", "amd", tutorial!("debian-amd")),
    ("debian", "nvidia", tutorial!("debian-nvidia")),
    ("ubuntu", "cpu", tutorial!("ubuntu-cpu")),
    ("ubuntu", "amd", tutorial!("ubuntu-amd")),
    ("ubuntu", "nvidia", tutorial!("ubuntu-nvidia")),
    ("dnf", "cpu", tutorial!("dnf-cpu")),
    ("apt", "cpu", tutorial!("apt-cpu")),
    ("pacman", "cpu", tutorial!("pacman-cpu")),
];

const GENERIC_TUTORIAL: &str = tutorial!("generic-cpu");

/// Selection order: `<distro>-<vendor>` (e.g. `fedora-nvidia`), then
/// `<distro>-cpu`, then `<pkg manager>-cpu`, then the fully generic
/// fallback. Pure — takes the already-detected distro id/package manager
/// rather than probing the system itself, so it's fully testable.
pub fn pick_tutorial(
    vendor: GpuVendor,
    distro_id: Option<&str>,
    pkg_mgr: Option<PackageManager>,
) -> &'static str {
    let distro_key = distro_id.filter(|id| KNOWN_DISTROS.contains(id));

    if let Some(distro) = distro_key {
        if let Some(suffix) = vendor.tutorial_suffix() {
            if let Some((_, _, text)) = TUTORIALS
                .iter()
                .find(|(d, v, _)| *d == distro && *v == suffix)
            {
                return text;
            }
        }
        if let Some((_, _, text)) = TUTORIALS
            .iter()
            .find(|(d, v, _)| *d == distro && *v == "cpu")
        {
            return text;
        }
    }

    if let Some(pm) = pkg_mgr {
        let prefix = pm.tutorial_prefix();
        if let Some((_, _, text)) = TUTORIALS
            .iter()
            .find(|(d, v, _)| *d == prefix && *v == "cpu")
        {
            return text;
        }
    }

    GENERIC_TUTORIAL
}

// ── Finding an already-installed libonnxruntime.so ───────────────────────

/// Known install locations across the distros/package managers this module
/// has tutorials for. Checked in order; the first that exists wins. This
/// only checks the file is *present* — actually loading it (`ort::init_from`)
/// is the real test, done by the caller (this just narrows candidates).
fn candidate_dylib_paths() -> Vec<PathBuf> {
    vec![
        // Fedora / RPM-based, x86_64
        PathBuf::from("/usr/lib64/libonnxruntime.so.1"),
        PathBuf::from("/usr/lib64/rocm/lib/libonnxruntime.so.1"),
        // Debian/Ubuntu multiarch, x86_64
        PathBuf::from("/usr/lib/x86_64-linux-gnu/libonnxruntime.so.1"),
        // Arch, x86_64
        PathBuf::from("/usr/lib/libonnxruntime.so.1"),
        // Generic fallback locations some distros/builds use.
        PathBuf::from("/usr/local/lib/libonnxruntime.so.1"),
        PathBuf::from("/usr/local/lib/libonnxruntime.so"),
    ]
}

/// The first candidate path that exists on disk, if any. Does not attempt
/// to load it — a caller wanting to know if it's actually *usable* should
/// try `ort::init_from()` on the result and fall back to the next candidate
/// (not implemented here — this module has no `ort` dependency) on failure.
pub fn find_onnxruntime_dylib() -> Option<PathBuf> {
    find_first_existing(&candidate_dylib_paths())
}

fn find_first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| p.is_file()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_vendor_id / best_vendor ────────────────────────────────

    #[test]
    fn classify_vendor_id_matches_known_ids() {
        assert_eq!(classify_vendor_id("0x10de"), GpuVendor::Nvidia);
        assert_eq!(classify_vendor_id("0x1002"), GpuVendor::Amd);
        assert_eq!(classify_vendor_id("0x8086"), GpuVendor::Intel);
        assert_eq!(classify_vendor_id("0xdead"), GpuVendor::Unknown);
    }

    #[test]
    fn classify_vendor_id_is_case_and_whitespace_insensitive() {
        assert_eq!(classify_vendor_id("0x10DE\n"), GpuVendor::Nvidia);
        assert_eq!(classify_vendor_id("  0x1002  "), GpuVendor::Amd);
    }

    #[test]
    fn best_vendor_prefers_discrete_over_intel() {
        assert_eq!(
            best_vendor([GpuVendor::Intel, GpuVendor::Nvidia]),
            GpuVendor::Nvidia
        );
        assert_eq!(
            best_vendor([GpuVendor::Intel, GpuVendor::Amd]),
            GpuVendor::Amd
        );
        assert_eq!(best_vendor([GpuVendor::Intel]), GpuVendor::Intel);
        assert_eq!(best_vendor([]), GpuVendor::Unknown);
    }

    // ── parse_os_release_id — real-world /etc/os-release samples ────────

    #[test]
    fn parse_os_release_id_fedora() {
        let content = "NAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID=44\n";
        assert_eq!(parse_os_release_id(content), Some("fedora".to_owned()));
    }

    #[test]
    fn parse_os_release_id_debian_unquoted() {
        let content = "PRETTY_NAME=\"Debian GNU/Linux\"\nID=debian\n";
        assert_eq!(parse_os_release_id(content), Some("debian".to_owned()));
    }

    #[test]
    fn parse_os_release_id_missing_returns_none() {
        assert_eq!(parse_os_release_id("NAME=\"Something\"\n"), None);
    }

    // ── pick_tutorial — selection order ─────────────────────────────────

    #[test]
    fn pick_tutorial_known_distro_and_vendor() {
        let t = pick_tutorial(GpuVendor::Nvidia, Some("fedora"), Some(PackageManager::Dnf));
        assert!(
            t.contains("onnxruntime-gpu"),
            "should be the fedora-nvidia tutorial"
        );
    }

    #[test]
    fn pick_tutorial_known_distro_unknown_vendor_falls_back_to_cpu() {
        let t = pick_tutorial(
            GpuVendor::Unknown,
            Some("fedora"),
            Some(PackageManager::Dnf),
        );
        assert!(t.contains("sudo dnf install onnxruntime"));
        assert!(!t.contains("onnxruntime-rocm"));
    }

    #[test]
    fn pick_tutorial_unknown_distro_falls_back_to_pkg_manager() {
        let t = pick_tutorial(
            GpuVendor::Unknown,
            Some("nobara"),
            Some(PackageManager::Dnf),
        );
        assert!(t.contains("sudo dnf install onnxruntime"));
    }

    #[test]
    fn pick_tutorial_nothing_detected_falls_back_to_generic() {
        let t = pick_tutorial(GpuVendor::Unknown, None, None);
        assert!(t.contains("pip install --user onnxruntime"));
    }

    #[test]
    fn pick_tutorial_arch_nvidia_mentions_the_conflict() {
        let t = pick_tutorial(
            GpuVendor::Nvidia,
            Some("arch"),
            Some(PackageManager::Pacman),
        );
        assert!(t.contains("conflicts"));
    }

    // ── find_first_existing ─────────────────────────────────────────────

    #[test]
    fn find_first_existing_returns_none_for_all_missing() {
        let paths = vec![
            PathBuf::from("/nonexistent/a.so"),
            PathBuf::from("/nonexistent/b.so"),
        ];
        assert_eq!(find_first_existing(&paths), None);
    }

    #[test]
    fn find_first_existing_skips_missing_entries() {
        // Cargo.toml always exists in the crate root at test time.
        let real = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let paths = vec![PathBuf::from("/nonexistent/a.so"), real.clone()];
        assert_eq!(find_first_existing(&paths), Some(real));
    }

    // ── live: real system detection (not #[ignore] — safe on any machine,
    // just asserts internal consistency rather than an exact platform) ──

    #[test]
    fn detect_functions_run_without_panicking_on_this_machine() {
        let _ = detect_gpu_vendor();
        let _ = detect_distro_id();
        let _ = detect_package_manager();
        let _ = find_onnxruntime_dylib();
    }
}
