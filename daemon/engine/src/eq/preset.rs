use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Band types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterType {
    LowShelf,
    Peaking,
    HighShelf,
}

/// A single EQ band.  Which fields are present depends on `BandMode`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    /// Gain in dB.  Range ±12 dB for LADSPA; ±10 dB for most HW targets.
    pub gain: f32,
    /// Centre/corner frequency in Hz.  Present only in `parametric_10` presets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency: Option<u16>,
    /// Filter shape.  Present only in `parametric_10` presets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_type: Option<FilterType>,
}

#[allow(dead_code)]
impl EqBand {
    pub fn gain_only(gain: f32) -> Self {
        Self {
            gain,
            frequency: None,
            filter_type: None,
        }
    }

    pub fn parametric(frequency: u16, filter_type: FilterType, gain: f32) -> Self {
        Self {
            gain,
            frequency: Some(frequency),
            filter_type: Some(filter_type),
        }
    }
}

// ── Band mode ─────────────────────────────────────────────────────────────────

/// How many bands a preset has, and whether per-band frequency is configurable.
///
/// Aligns with the three hardware EQ families in the device specs:
/// - `fixed_10`:      Nova Pro family (10 bands, fixed frequencies, gain only)
/// - `parametric_10`: Nova 3/5/7 Gen2/Elite family (10 bands, free frequency + filter)
/// - `fixed_5`:       Arctis 5 (5 bands, fixed frequencies, gain only)
///
/// LADSPA `mbeq_1197` can serve all three modes (software fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandMode {
    Fixed10,
    Parametric10,
    Fixed5,
}

impl BandMode {
    pub fn band_count(self) -> usize {
        match self {
            BandMode::Fixed10 | BandMode::Parametric10 => 10,
            BandMode::Fixed5 => 5,
        }
    }

    /// Whether bands require a frequency value.
    pub fn requires_frequency(self) -> bool {
        matches!(self, BandMode::Parametric10)
    }
}

// ── EQ preset ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqPreset {
    pub name: String,
    pub band_mode: BandMode,
    pub bands: Vec<EqBand>,
}

impl EqPreset {
    /// Returns `Err` if band count or required fields don't match `band_mode`.
    pub fn validate(&self) -> Result<(), String> {
        let expected = self.band_mode.band_count();
        if self.bands.len() != expected {
            return Err(format!(
                "preset '{}': expected {expected} bands, got {}",
                self.name,
                self.bands.len()
            ));
        }
        if self.band_mode.requires_frequency() {
            for (i, b) in self.bands.iter().enumerate() {
                if b.frequency.is_none() {
                    return Err(format!(
                        "preset '{}': band {i} missing frequency (required for parametric_10)",
                        self.name
                    ));
                }
                if b.filter_type.is_none() {
                    return Err(format!(
                        "preset '{}': band {i} missing filter_type (required for parametric_10)",
                        self.name
                    ));
                }
            }
        }
        Ok(())
    }
}

// ── Flat (0 dB) preset constructors ──────────────────────────────────────────

#[allow(dead_code)]
pub fn flat_preset(band_mode: BandMode) -> EqPreset {
    let bands = match band_mode {
        BandMode::Fixed10 | BandMode::Parametric10 => {
            let freqs: [u16; 10] = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
            if band_mode == BandMode::Parametric10 {
                freqs
                    .iter()
                    .map(|&f| EqBand::parametric(f, FilterType::Peaking, 0.0))
                    .collect()
            } else {
                (0..10).map(|_| EqBand::gain_only(0.0)).collect()
            }
        }
        BandMode::Fixed5 => (0..5).map(|_| EqBand::gain_only(0.0)).collect(),
    };
    EqPreset {
        name: "Flat".to_owned(),
        band_mode,
        bands,
    }
}

// ── Persistence ───────────────────────────────────────────────────────────────

pub fn preset_path(base_dir: &Path, name: &str) -> PathBuf {
    // Sanitise: replace filesystem-unsafe characters with '_'.
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    base_dir.join("eq_presets").join(format!("{safe}.yaml"))
}

pub fn save_preset(base_dir: &Path, preset: &EqPreset) -> std::io::Result<()> {
    preset.validate().map_err(std::io::Error::other)?;
    let path = preset_path(base_dir, &preset.name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let yaml = serde_yaml::to_string(preset).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, yaml)
}

pub fn load_preset(path: &Path) -> Result<EqPreset, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let preset: EqPreset =
        serde_yaml::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))?;
    preset.validate()?;
    Ok(preset)
}

/// List all valid presets in `<base_dir>/eq_presets/`.
/// Silently skips files that fail to parse or validate.
pub fn list_presets(base_dir: &Path) -> Vec<EqPreset> {
    let dir = base_dir.join("eq_presets");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut presets: Vec<EqPreset> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension()?.to_str()? != "yaml" {
                return None;
            }
            load_preset(&path).ok()
        })
        .collect();
    presets.sort_by(|a, b| a.name.cmp(&b.name));
    presets
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── BandMode ──────────────────────────────────────────────────────────────

    #[test]
    fn band_mode_counts() {
        assert_eq!(BandMode::Fixed10.band_count(), 10);
        assert_eq!(BandMode::Parametric10.band_count(), 10);
        assert_eq!(BandMode::Fixed5.band_count(), 5);
    }

    #[test]
    fn band_mode_requires_frequency_only_parametric() {
        assert!(!BandMode::Fixed10.requires_frequency());
        assert!(BandMode::Parametric10.requires_frequency());
        assert!(!BandMode::Fixed5.requires_frequency());
    }

    // ── EqPreset::validate ────────────────────────────────────────────────────

    #[test]
    fn validate_fixed10_wrong_count() {
        let preset = EqPreset {
            name: "bad".into(),
            band_mode: BandMode::Fixed10,
            bands: vec![EqBand::gain_only(0.0); 5],
        };
        assert!(preset.validate().is_err());
    }

    #[test]
    fn validate_parametric10_missing_frequency() {
        let mut bands: Vec<EqBand> = (0..10)
            .map(|_| EqBand::parametric(1000, FilterType::Peaking, 0.0))
            .collect();
        bands[3].frequency = None;
        let preset = EqPreset {
            name: "p".into(),
            band_mode: BandMode::Parametric10,
            bands,
        };
        assert!(preset.validate().is_err());
    }

    #[test]
    fn validate_parametric10_missing_filter_type() {
        let mut bands: Vec<EqBand> = (0..10)
            .map(|_| EqBand::parametric(1000, FilterType::Peaking, 0.0))
            .collect();
        bands[5].filter_type = None;
        let preset = EqPreset {
            name: "p".into(),
            band_mode: BandMode::Parametric10,
            bands,
        };
        assert!(preset.validate().is_err());
    }

    #[test]
    fn validate_fixed5_ok() {
        let preset = EqPreset {
            name: "five".into(),
            band_mode: BandMode::Fixed5,
            bands: vec![EqBand::gain_only(1.0); 5],
        };
        assert!(preset.validate().is_ok());
    }

    #[test]
    fn validate_fixed5_wrong_count() {
        let preset = EqPreset {
            name: "five".into(),
            band_mode: BandMode::Fixed5,
            bands: vec![EqBand::gain_only(1.0); 10],
        };
        assert!(preset.validate().is_err());
    }

    // ── flat_preset ───────────────────────────────────────────────────────────

    #[test]
    fn flat_fixed10_is_valid_all_zero() {
        let p = flat_preset(BandMode::Fixed10);
        assert_eq!(p.bands.len(), 10);
        assert!(p.validate().is_ok());
        assert!(p
            .bands
            .iter()
            .all(|b| b.gain == 0.0 && b.frequency.is_none()));
    }

    #[test]
    fn flat_parametric10_has_frequencies_and_filter_types() {
        let p = flat_preset(BandMode::Parametric10);
        assert_eq!(p.bands.len(), 10);
        assert!(p.validate().is_ok());
        assert!(p
            .bands
            .iter()
            .all(|b| b.frequency.is_some() && b.filter_type.is_some()));
    }

    #[test]
    fn flat_fixed5_is_valid_all_zero() {
        let p = flat_preset(BandMode::Fixed5);
        assert_eq!(p.bands.len(), 5);
        assert!(p.validate().is_ok());
        assert!(p.bands.iter().all(|b| b.gain == 0.0));
    }

    // ── YAML round-trip ───────────────────────────────────────────────────────

    #[test]
    fn fixed10_yaml_roundtrip() {
        let original = EqPreset {
            name: "Bass Boost".into(),
            band_mode: BandMode::Fixed10,
            bands: [4.0f32, 3.0, 2.0, 1.0, 0.0, 0.0, -1.0, -2.0, -3.0, -4.0]
                .iter()
                .map(|&g| EqBand::gain_only(g))
                .collect(),
        };
        let yaml = serde_yaml::to_string(&original).unwrap();
        let loaded: EqPreset = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(loaded, original);
        assert!(loaded.validate().is_ok());
    }

    #[test]
    fn parametric10_yaml_roundtrip() {
        let freqs: [u16; 10] = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
        let bands: Vec<EqBand> = freqs
            .iter()
            .enumerate()
            .map(|(i, &f)| {
                let ft = if i == 0 {
                    FilterType::LowShelf
                } else if i == 9 {
                    FilterType::HighShelf
                } else {
                    FilterType::Peaking
                };
                EqBand::parametric(f, ft, i as f32 * 0.5)
            })
            .collect();
        let original = EqPreset {
            name: "Parametric test".into(),
            band_mode: BandMode::Parametric10,
            bands,
        };
        let yaml = serde_yaml::to_string(&original).unwrap();
        let loaded: EqPreset = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(loaded, original);
        assert!(loaded.validate().is_ok());
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    #[test]
    fn save_and_load_preset_roundtrip() {
        let dir = tempdir().unwrap();
        let preset = flat_preset(BandMode::Fixed10);
        save_preset(dir.path(), &preset).unwrap();
        let path = preset_path(dir.path(), "Flat");
        let loaded = load_preset(&path).unwrap();
        assert_eq!(loaded, preset);
    }

    #[test]
    fn preset_path_sanitises_name() {
        let base = Path::new("/cfg");
        let p = preset_path(base, "Bass/Boost & Treble");
        assert_eq!(p, Path::new("/cfg/eq_presets/Bass_Boost___Treble.yaml"));
    }

    #[test]
    fn list_presets_returns_sorted_valid_only() {
        let dir = tempdir().unwrap();
        save_preset(dir.path(), &flat_preset(BandMode::Fixed10)).unwrap();
        let p5 = EqPreset {
            name: "Arctis5".into(),
            band_mode: BandMode::Fixed5,
            bands: vec![EqBand::gain_only(1.0); 5],
        };
        save_preset(dir.path(), &p5).unwrap();
        // Write an invalid YAML file — should be silently skipped.
        let bad = dir.path().join("eq_presets/broken.yaml");
        std::fs::write(&bad, b": not: yaml: [").unwrap();

        let presets = list_presets(dir.path());
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].name, "Arctis5");
        assert_eq!(presets[1].name, "Flat");
    }

    #[test]
    fn list_presets_returns_empty_when_dir_absent() {
        let dir = tempdir().unwrap();
        let presets = list_presets(dir.path());
        assert!(presets.is_empty());
    }

    #[test]
    fn save_preset_fails_on_invalid_preset() {
        let dir = tempdir().unwrap();
        let bad = EqPreset {
            name: "bad".into(),
            band_mode: BandMode::Fixed10,
            bands: vec![],
        };
        assert!(save_preset(dir.path(), &bad).is_err());
    }
}
