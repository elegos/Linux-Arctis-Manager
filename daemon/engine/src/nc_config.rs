use serde::{Deserialize, Serialize};

// LADSPA plugin filenames (without .so) and labels.
pub const RNNOISE_PLUGIN: &str = "librnnoise_ladspa";
pub const RNNOISE_PLUGIN_ALT: &str = "rnnoise_ladspa";
pub const RNNOISE_LABEL: &str = "noise_suppressor_mono";

// VAD threshold %, grace ms, retroactive grace ms — same tuning as Python reference.
pub const RNNOISE_CONTROLS: (f64, f64, f64) = (15.0, 350.0, 0.0);

pub const GATE_CANDIDATES: &[(&str, &str)] = &[("gate_1410", "gate"), ("gate", "gate")];
// sc4m is the mono compressor; sc4 is stereo-only and breaks single-channel filter-chain graphs.
pub const COMP_CANDIDATES: &[(&str, &str)] = &[("sc4m_1916", "sc4m"), ("sc4m", "sc4m")];

/// Noise gate parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GateConfig {
    pub enabled: bool,
    pub threshold: i32,
    pub reduction: i32,
    pub attack: u32,
    pub release: u32,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: -42,
            reduction: -72,
            attack: 2,
            release: 450,
        }
    }
}

/// Compressor parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressorConfig {
    pub enabled: bool,
    pub threshold: i32,
    /// Stored as 10× (e.g. 18 → ratio 1.8:1).
    pub ratio: u32,
    pub makeup: u32,
}

impl Default for CompressorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: -18,
            ratio: 18,
            makeup: 4,
        }
    }
}

/// Top-level NC configuration persisted and sent over D-Bus as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NcConfig {
    /// "off" disables NC entirely; any other value (light/standard/studio/
    /// custom) enables it and is always active — unlike VC, NC has no
    /// separate autostart/session-only tri-state: the persisted preset
    /// *is* the desired state, and is always re-applied on daemon startup.
    pub preset: String,
    /// Stable ALSA `node.name` of the physical mic source to process.
    pub source_id: String,
    pub hpf_enabled: bool,
    pub gate: GateConfig,
    pub compressor: CompressorConfig,
}

impl Default for NcConfig {
    fn default() -> Self {
        Self {
            preset: "off".to_owned(),
            source_id: String::new(),
            hpf_enabled: false,
            gate: GateConfig::default(),
            compressor: CompressorConfig::default(),
        }
    }
}

impl NcConfig {
    pub fn active(&self) -> bool {
        self.preset != "off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off() {
        let cfg = NcConfig::default();
        assert!(!cfg.active());
        assert_eq!(cfg.preset, "off");
    }

    #[test]
    fn autostart_field_removed_from_old_json_is_ignored() {
        // Old persisted configs written before `autostart` was removed —
        // `#[serde(default)]` (via `unknown_future_field`-style tolerance)
        // must still parse, simply dropping the now-unknown field.
        let cfg: NcConfig = serde_json::from_str(r#"{"preset": "on", "autostart": true}"#)
            .unwrap();
        assert_eq!(cfg.preset, "on");
    }

    #[test]
    fn active_when_preset_is_not_off() {
        let cfg = NcConfig {
            preset: "on".to_owned(),
            source_id: "alsa_input.usb-foo".to_owned(),
            ..Default::default()
        };
        assert!(cfg.active());
    }

    #[test]
    fn roundtrip_default() {
        let cfg = NcConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: NcConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn roundtrip_full_config() {
        let cfg = NcConfig {
            preset: "custom".to_owned(),
            source_id: "alsa_input.usb-SteelSeries-00.mono-fallback".to_owned(),
            hpf_enabled: true,
            gate: GateConfig {
                enabled: true,
                threshold: -36,
                reduction: -60,
                attack: 5,
                release: 300,
            },
            compressor: CompressorConfig {
                enabled: true,
                threshold: -20,
                ratio: 30,
                makeup: 6,
            },
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: NcConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"{"preset":"on","source_id":"foo","unknown_future_field":42}"#;
        let cfg: NcConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.preset, "on");
        assert_eq!(cfg.source_id, "foo");
    }

    #[test]
    fn missing_fields_use_defaults() {
        let json = r#"{"preset":"on"}"#;
        let cfg: NcConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.active());
        assert_eq!(cfg.gate, GateConfig::default());
        assert_eq!(cfg.compressor, CompressorConfig::default());
    }

    #[test]
    fn gate_defaults() {
        let g = GateConfig::default();
        assert!(!g.enabled);
        assert_eq!(g.threshold, -42);
        assert_eq!(g.reduction, -72);
    }

    #[test]
    fn compressor_defaults() {
        let c = CompressorConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.threshold, -18);
        assert_eq!(c.ratio, 18);
        assert_eq!(c.makeup, 4);
    }
}
