// LADSPA `mbeq_1197` EQ pipeline management via PipeWire/PulseAudio.
//
// `mbeq_1197` is a fixed-frequency 15-band EQ.  All three band modes
// (fixed_10, parametric_10, fixed_5) are supported as a complete, independent
// pipeline — the band mode determines how user gains are mapped onto the 15
// LADSPA control ports.  The pipeline is always available regardless of device
// hardware capability.
//
// Pure functions (gain mapping, argument formatting) are unit-tested.
// I/O functions (module load/unload, live gain update) are covered by the
// `lam-integrity-check ladspa-eq` subcommand.


use super::preset::{BandMode, EqPreset};

// ── Plugin geometry ───────────────────────────────────────────────────────────

/// Centre frequencies of the 15 `mbeq_1197` control ports, in order.
/// These are fixed by the plugin and must not change.
pub const MBEQ_FREQ: [f32; 15] = [
    50.0, 100.0, 156.0, 220.0, 311.0, 440.0, 622.0, 880.0, 1250.0, 1750.0, 2500.0, 3500.0, 5000.0,
    10000.0, 20000.0,
];

/// Indices into `MBEQ_FREQ` activated by `BandMode::Fixed10`.
/// Selected for even log-frequency coverage across the audible range.
/// Order matches the user-facing band order (bass → treble).
pub const FIXED_10_INDICES: [usize; 10] = [0, 1, 3, 5, 7, 9, 11, 12, 13, 14];
// → 50, 100, 220, 440, 880, 1750, 3500, 5000, 10000, 20000 Hz

/// Indices into `MBEQ_FREQ` activated by `BandMode::Fixed5`.
pub const FIXED_5_INDICES: [usize; 5] = [0, 5, 9, 12, 14];
// → 50, 440, 1750, 5000, 20000 Hz

// ── Gain mapping ──────────────────────────────────────────────────────────────

/// Map preset bands onto the 15 `mbeq_1197` control gains.
///
/// - `fixed_10`: the i-th user band maps to `FIXED_10_INDICES[i]`; others = 0.
/// - `fixed_5`:  the i-th user band maps to `FIXED_5_INDICES[i]`; others = 0.
/// - `parametric_10`: each band maps to the nearest mbeq frequency (log
///   distance); if two bands hit the same mbeq slot, their gains are summed.
///   Bands with no `frequency` field default to 1000 Hz.
pub fn gains_for_preset(preset: &EqPreset) -> [f32; 15] {
    let mut gains = [0.0f32; 15];
    match preset.band_mode {
        BandMode::Fixed10 => {
            for (i, band) in preset.bands.iter().take(10).enumerate() {
                gains[FIXED_10_INDICES[i]] = band.gain;
            }
        }
        BandMode::Fixed5 => {
            for (i, band) in preset.bands.iter().take(5).enumerate() {
                gains[FIXED_5_INDICES[i]] = band.gain;
            }
        }
        BandMode::Parametric10 => {
            for band in &preset.bands {
                let freq = band.frequency.unwrap_or(1000) as f32;
                let idx = nearest_mbeq_index(freq);
                gains[idx] += band.gain;
            }
        }
    }
    gains
}

/// Return the index of the `mbeq_1197` band whose frequency is closest to
/// `freq` in log space.
pub fn nearest_mbeq_index(freq: f32) -> usize {
    let log_f = freq.max(1.0).ln();
    MBEQ_FREQ
        .iter()
        .enumerate()
        .min_by(|(_, &a), (_, &b)| {
            let da = (a.ln() - log_f).abs();
            let db = (b.ln() - log_f).abs();
            da.partial_cmp(&db).unwrap()
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Format the 15 gains as the `control=` argument for `module-ladspa-sink`.
pub fn control_arg(gains: &[f32; 15]) -> String {
    gains
        .iter()
        .map(|g| format!("{g:.2}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Build the full argument string for `pactl load-module module-ladspa-sink`.
///
/// `sink_name` — the new virtual sink name (e.g. `"Arctis_Media_EQ_internal"`).
/// `master`    — the sink to tap from (e.g. `"Arctis_Media"`).
/// `gains`     — the 15 mbeq_1197 control gains.
pub fn load_module_args(sink_name: &str, master: &str, gains: &[f32; 15]) -> String {
    format!(
        "sink_name={sink_name} \
         sink_master={master} \
         label=mbeq \
         plugin=mbeq_1197 \
         control={}",
        control_arg(gains)
    )
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LadspaError {
    Pactl(String),
    PwCli(String),
    PluginNotFound,
    NodeNotFound(String),
}

impl std::fmt::Display for LadspaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pactl(e) => write!(f, "pactl: {e}"),
            Self::PwCli(e) => write!(f, "pw-cli: {e}"),
            Self::PluginNotFound => write!(f, "mbeq_1197 LADSPA plugin not installed"),
            Self::NodeNotFound(n) => write!(f, "PipeWire node not found: {n}"),
        }
    }
}

// ── I/O helpers ───────────────────────────────────────────────────────────────

async fn pactl(args: &[&str]) -> Result<String, LadspaError> {
    let out = tokio::process::Command::new("pactl")
        .args(args)
        .output()
        .await
        .map_err(|e| LadspaError::Pactl(e.to_string()))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(LadspaError::Pactl(msg));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn pw_cli(args: &[&str]) -> Result<String, LadspaError> {
    let out = tokio::process::Command::new("pw-cli")
        .args(args)
        .output()
        .await
        .map_err(|e| LadspaError::PwCli(e.to_string()))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(LadspaError::PwCli(msg));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ── Public async API ──────────────────────────────────────────────────────────

/// Verify that the `mbeq_1197` LADSPA plugin is installed on this system.
pub async fn check_plugin_available() -> bool {
    // Split LADSPA_PATH on ':' and search each directory separately.
    let ladspa_path = std::env::var("LADSPA_PATH")
        .unwrap_or_else(|_| "/usr/lib/ladspa:/usr/lib64/ladspa:/usr/local/lib/ladspa".to_owned());
    for dir in ladspa_path.split(':') {
        let candidate = std::path::Path::new(dir).join("mbeq_1197.so");
        if candidate.exists() {
            return true;
        }
    }
    false
}

/// Load a `module-ladspa-sink` for one channel and return the module index.
pub async fn load_eq_module(
    sink_name: &str,
    master: &str,
    gains: &[f32; 15],
) -> Result<u32, LadspaError> {
    let args = load_module_args(sink_name, master, gains);
    let out = pactl(&["load-module", "module-ladspa-sink", &args]).await?;
    out.trim()
        .parse::<u32>()
        .map_err(|_| LadspaError::Pactl(format!("unexpected load-module output: {out}")))
}

/// Unload a previously loaded `module-ladspa-sink` by its module index.
pub async fn unload_eq_module(module_id: u32) -> Result<(), LadspaError> {
    pactl(&["unload-module", &module_id.to_string()])
        .await
        .map(|_| ())
}

/// Find the PipeWire node ID for a named sink by parsing `pw-dump` JSON.
/// Returns `None` when no node with that `node.name` property exists.
pub async fn find_pw_node_id(sink_name: &str) -> Option<u32> {
    let out = pw_cli(&["dump", "short"]).await.ok()?;
    // pw-cli dump short prints lines like: <id> <type> ...
    // We need pw-dump (JSON) for property lookup.
    drop(out); // pw-cli dump short doesn't give properties; use pw-dump instead.

    let json_out = tokio::process::Command::new("pw-dump")
        .output()
        .await
        .ok()?;
    if !json_out.status.success() {
        return None;
    }
    let json_str = String::from_utf8_lossy(&json_out.stdout);
    let nodes: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    for node in nodes.as_array()? {
        let props = &node["info"]["props"];
        if props["node.name"].as_str() == Some(sink_name) {
            if let Some(id) = node["id"].as_u64() {
                return Some(id as u32);
            }
        }
    }
    None
}

/// Push updated gains to an already-running `module-ladspa-sink` without
/// reloading it (no audio interruption).
///
/// Uses `pw-cli set-param` to update the LADSPA control ports live.
pub async fn update_gains_live(sink_name: &str, gains: &[f32; 15]) -> Result<(), LadspaError> {
    let node_id = find_pw_node_id(sink_name)
        .await
        .ok_or_else(|| LadspaError::NodeNotFound(sink_name.to_owned()))?;

    // Build the SPA JSON props update for the LADSPA control array.
    let gain_arr: String = gains
        .iter()
        .map(|g| format!("{g:.2}"))
        .collect::<Vec<_>>()
        .join(" ");
    let props = format!("{{ params = [ \"ladspa.control\" [ {gain_arr} ] ] }}");
    pw_cli(&["s", &node_id.to_string(), "Props", &props])
        .await
        .map(|_| ())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::preset::{EqBand, FilterType};

    // ── nearest_mbeq_index ────────────────────────────────────────────────────

    #[test]
    fn nearest_mbeq_exact_match_50hz() {
        assert_eq!(nearest_mbeq_index(50.0), 0);
    }

    #[test]
    fn nearest_mbeq_exact_match_20000hz() {
        assert_eq!(nearest_mbeq_index(20000.0), 14);
    }

    #[test]
    fn nearest_mbeq_midpoint_picks_closer_in_log_space() {
        // Between 50 and 100 Hz, geometric mean is √(50×100) ≈ 70.7 Hz.
        // Below the midpoint → 50 Hz (index 0).
        assert_eq!(nearest_mbeq_index(65.0), 0);
        // Above the midpoint → 100 Hz (index 1).
        assert_eq!(nearest_mbeq_index(80.0), 1);
    }

    #[test]
    fn nearest_mbeq_1000hz_picks_880hz() {
        // 880 (index 7) vs 1250 (index 8): geometric mean ≈ 1049.
        assert_eq!(nearest_mbeq_index(1000.0), 7);
    }

    #[test]
    fn nearest_mbeq_1100hz_picks_1250hz() {
        assert_eq!(nearest_mbeq_index(1100.0), 8);
    }

    // ── gains_for_preset — fixed_10 ───────────────────────────────────────────

    #[test]
    fn fixed10_flat_all_mbeq_zero() {
        let preset = EqPreset {
            name: "Flat".into(),
            band_mode: BandMode::Fixed10,
            bands: vec![EqBand::gain_only(0.0); 10],
        };
        let gains = gains_for_preset(&preset);
        assert!(gains.iter().all(|&g| g == 0.0));
    }

    #[test]
    fn fixed10_first_band_maps_to_index_0() {
        let mut bands = vec![EqBand::gain_only(0.0); 10];
        bands[0].gain = 3.0;
        let preset = EqPreset {
            name: "test".into(),
            band_mode: BandMode::Fixed10,
            bands,
        };
        let gains = gains_for_preset(&preset);
        assert_eq!(gains[FIXED_10_INDICES[0]], 3.0);
        // All other indices = 0
        for (i, &g) in gains.iter().enumerate() {
            if i != FIXED_10_INDICES[0] {
                assert_eq!(g, 0.0, "expected 0 at mbeq index {i}");
            }
        }
    }

    #[test]
    fn fixed10_last_band_maps_to_20khz() {
        let mut bands = vec![EqBand::gain_only(0.0); 10];
        bands[9].gain = -6.0;
        let preset = EqPreset {
            name: "test".into(),
            band_mode: BandMode::Fixed10,
            bands,
        };
        let gains = gains_for_preset(&preset);
        assert_eq!(gains[FIXED_10_INDICES[9]], -6.0);
        assert_eq!(MBEQ_FREQ[FIXED_10_INDICES[9]], 20000.0);
    }

    #[test]
    fn fixed10_indices_cover_full_range() {
        // First index should be the lowest frequency, last the highest.
        let first = MBEQ_FREQ[FIXED_10_INDICES[0]];
        let last = MBEQ_FREQ[FIXED_10_INDICES[9]];
        assert!(first < 100.0, "first band freq {first} not in bass range");
        assert!(last > 15000.0, "last band freq {last} not in air range");
    }

    // ── gains_for_preset — fixed_5 ────────────────────────────────────────────

    #[test]
    fn fixed5_maps_correctly() {
        let bands: Vec<EqBand> = [1.0f32, 2.0, 3.0, 4.0, 5.0]
            .iter()
            .map(|&g| EqBand::gain_only(g))
            .collect();
        let preset = EqPreset {
            name: "test".into(),
            band_mode: BandMode::Fixed5,
            bands,
        };
        let gains = gains_for_preset(&preset);
        for (i, &idx) in FIXED_5_INDICES.iter().enumerate() {
            assert_eq!(
                gains[idx],
                (i + 1) as f32,
                "mbeq[{idx}] should be {}",
                i + 1
            );
        }
        // Non-active indices = 0
        for (i, &g) in gains.iter().enumerate() {
            if !FIXED_5_INDICES.contains(&i) {
                assert_eq!(g, 0.0, "mbeq[{i}] should be 0");
            }
        }
    }

    #[test]
    fn fixed5_indices_no_overlap() {
        let mut seen = [false; 15];
        for &i in &FIXED_5_INDICES {
            assert!(!seen[i], "duplicate index {i} in FIXED_5_INDICES");
            seen[i] = true;
        }
    }

    // ── gains_for_preset — parametric_10 ─────────────────────────────────────

    #[test]
    fn parametric10_maps_to_nearest_frequency() {
        let bands = vec![EqBand::parametric(50, FilterType::LowShelf, 4.0)];
        let preset = EqPreset {
            name: "test".into(),
            band_mode: BandMode::Parametric10,
            bands,
        };
        let gains = gains_for_preset(&preset);
        // 50 Hz → exact match at index 0
        assert_eq!(gains[0], 4.0);
    }

    #[test]
    fn parametric10_two_bands_same_slot_sums_gains() {
        // Both 50 Hz and 60 Hz map to index 0 (nearest to 50 Hz).
        let bands = vec![
            EqBand::parametric(50, FilterType::Peaking, 2.0),
            EqBand::parametric(60, FilterType::Peaking, 1.5),
        ];
        let preset = EqPreset {
            name: "test".into(),
            band_mode: BandMode::Parametric10,
            bands,
        };
        let gains = gains_for_preset(&preset);
        assert!((gains[0] - 3.5).abs() < 1e-5);
    }

    #[test]
    fn parametric10_missing_frequency_defaults_to_1000hz() {
        let band = EqBand {
            gain: 2.0,
            frequency: None,
            filter_type: None,
        };
        let preset = EqPreset {
            name: "test".into(),
            band_mode: BandMode::Parametric10,
            bands: vec![band],
        };
        let gains = gains_for_preset(&preset);
        let expected_idx = nearest_mbeq_index(1000.0);
        assert_eq!(gains[expected_idx], 2.0);
    }

    // ── control_arg formatting ────────────────────────────────────────────────

    #[test]
    fn control_arg_flat_all_zeros() {
        let gains = [0.0f32; 15];
        let s = control_arg(&gains);
        assert_eq!(
            s,
            "0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00"
        );
    }

    #[test]
    fn control_arg_with_values() {
        let mut gains = [0.0f32; 15];
        gains[0] = 3.5;
        gains[14] = -6.0;
        let s = control_arg(&gains);
        let parts: Vec<&str> = s.split(',').collect();
        assert_eq!(parts.len(), 15);
        assert_eq!(parts[0], "3.50");
        assert_eq!(parts[14], "-6.00");
    }

    // ── load_module_args ──────────────────────────────────────────────────────

    #[test]
    fn load_module_args_contains_required_fields() {
        let gains = [0.0f32; 15];
        let args = load_module_args("Arctis_Media_EQ_internal", "Arctis_Media", &gains);
        assert!(args.contains("sink_name=Arctis_Media_EQ_internal"));
        assert!(args.contains("sink_master=Arctis_Media"));
        assert!(args.contains("label=mbeq"));
        assert!(args.contains("plugin=mbeq_1197"));
        assert!(args.contains("control="));
    }

    // ── FIXED_10_INDICES and FIXED_5_INDICES consistency ─────────────────────

    #[test]
    fn fixed10_indices_all_in_range() {
        for &i in &FIXED_10_INDICES {
            assert!(i < 15, "index {i} out of range");
        }
    }

    #[test]
    fn fixed10_indices_no_duplicates() {
        let mut seen = [false; 15];
        for &i in &FIXED_10_INDICES {
            assert!(!seen[i], "duplicate index {i}");
            seen[i] = true;
        }
    }

    #[test]
    fn fixed5_indices_all_in_range() {
        for &i in &FIXED_5_INDICES {
            assert!(i < 15);
        }
    }

    #[test]
    fn mbeq_freq_is_sorted_ascending() {
        for w in MBEQ_FREQ.windows(2) {
            assert!(w[0] < w[1], "MBEQ_FREQ not sorted: {} >= {}", w[0], w[1]);
        }
    }
}
