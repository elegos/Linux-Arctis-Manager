// Shared LADSPA plugin discovery, used by both the NC and VC filter-chain
// managers (`nc_manager.rs`, `vc_ladspa_chain.rs`).

use std::path::PathBuf;

const LADSPA_SEARCH_PATHS: &[&str] = &[
    "/usr/lib/ladspa",
    "/usr/lib64/ladspa",
    "/usr/local/lib/ladspa",
    "/usr/local/lib64/ladspa",
];

fn ladspa_search_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::env::var("LADSPA_PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    paths.extend(LADSPA_SEARCH_PATHS.iter().map(PathBuf::from));
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        paths.push(PathBuf::from(home).join(".ladspa"));
    }
    paths
}

pub fn plugin_available(name: &str) -> bool {
    let filename = format!("{name}.so");
    ladspa_search_paths()
        .iter()
        .any(|d| d.join(&filename).is_file())
}

/// Return the first `(plugin, label)` candidate whose `.so` is found on this system.
pub fn find_plugin(
    candidates: &[(&'static str, &'static str)],
) -> Option<(&'static str, &'static str)> {
    candidates
        .iter()
        .find(|(p, _)| plugin_available(p))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_available_false_for_unknown_plugin() {
        assert!(!plugin_available("definitely_not_a_real_ladspa_plugin_xyz"));
    }

    #[test]
    fn find_plugin_returns_none_when_no_candidate_present() {
        let candidates: &[(&'static str, &'static str)] =
            &[("definitely_not_a_real_ladspa_plugin_xyz", "label")];
        assert!(find_plugin(candidates).is_none());
    }
}
