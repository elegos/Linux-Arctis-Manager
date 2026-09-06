use std::path::Path;
use std::process::Command;

fn main() {
    let version_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());

    let raw = std::fs::read_to_string(&version_file)
        .unwrap_or_else(|_| panic!("cannot read {}", version_file.display()));
    let version = raw.trim();
    assert!(!version.is_empty(), "VERSION file is empty");

    println!("cargo:rustc-env=LAM_VERSION={version}");

    // Best-effort short git commit hash, surfaced in the startup log line
    // alongside LAM_VERSION. During active development VERSION is bumped per
    // release, not per commit, so two builds days apart routinely share the
    // same LAM_VERSION — that ambiguity has repeatedly cost real
    // back-and-forth confirming whether a bug reporter is actually running a
    // just-fixed build. Falls back to "unknown" when building from a source
    // tarball with no .git directory, or git isn't on PATH — never fails the
    // build over this, and deliberately has no rerun-if-changed on .git
    // internals (fragile to track precisely); a packaging rebuild always
    // starts this script fresh anyway.
    let commit = Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LAM_GIT_COMMIT={commit}");
}
