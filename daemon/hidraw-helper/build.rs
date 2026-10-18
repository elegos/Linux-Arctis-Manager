use std::path::Path;

fn main() {
    let version_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());

    let raw = std::fs::read_to_string(&version_file)
        .unwrap_or_else(|_| panic!("cannot read {}", version_file.display()));
    let version = raw.trim();
    assert!(!version.is_empty(), "VERSION file is empty");

    println!("cargo:rustc-env=LAM_VERSION={version}");

    write_accessory_pids();
}

/// Compiles the PIDs of every `companion` device config (pure-HID accessories
/// of an audio product, such as the GameBuds charging case) into
/// `AUDIO_ACCESSORY_PIDS`. The privileged helper does not read configs at
/// runtime: the allowlist is fixed at build time from the shipped configs, so
/// it cannot drift from them.
fn write_accessory_pids() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../device-configs");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("base_"))
        })
        .collect();
    paths.sort();

    let mut pids: Vec<u16> = Vec::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let cfg = device_config::load(&path, &[dir.as_path()])
            .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
        let Some(device) = cfg.device.filter(|d| d.companion) else {
            continue;
        };
        pids.extend(device.variants.iter().flatten().map(|v| v.product_id));
    }
    pids.sort_unstable();
    pids.dedup();

    let list: Vec<String> = pids.iter().map(|p| format!("{p:#06x}")).collect();
    let out =
        Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("audio_accessory_pids.rs");
    std::fs::write(
        out,
        format!(
            "const AUDIO_ACCESSORY_PIDS: &[u16] = &[{}];\n",
            list.join(", ")
        ),
    )
    .expect("write audio_accessory_pids.rs");
}
