use std::path::Path;

fn main() {
    let version_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());

    let raw = std::fs::read_to_string(&version_file)
        .unwrap_or_else(|_| panic!("cannot read {}", version_file.display()));
    let version = raw.trim();
    assert!(!version.is_empty(), "VERSION file is empty");

    println!("cargo:rustc-env=LAM_VERSION={version}");
}
