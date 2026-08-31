// Voice Changer settings — LADSPA effect chain configuration.
//
// Field set and defaults are a direct port of the Python reference
// (`voice_changer/settings.py` / `voice_changer/ladspa/effects.py`). The RVC
// (neural voice conversion) config is a separate, later addition — see
// docs/voice-changing-feature.md.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::vc_rvc_config::RvcParams;

// LADSPA plugin filenames (without .so) and labels, in preference order.
pub const PITCH_CANDIDATES: &[(&str, &str)] = &[
    ("am_pitchshift_1433", "amPitchshift"),
    ("pitch_scale_1193", "pitchScale"),
];
pub const CHORUS_CANDIDATES: &[(&str, &str)] = &[("multivoice_chorus_1201", "multivoiceChorus")];
pub const DELAY_CANDIDATES: &[(&str, &str)] = &[("delay_1898", "delay_n")];
pub const DISTORTION_CANDIDATES: &[(&str, &str)] = &[("valve_1209", "valve")];
pub const REVERB_CANDIDATES: &[(&str, &str)] = &[("gverb_1216", "gverb")];

/// Pitch shift — `am_pitchshift_1433` (preferred) or `pitch_scale_1193`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PitchConfig {
    pub enabled: bool,
    /// -24..+24 semitones.
    pub semitones: f32,
}

impl Default for PitchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            semitones: 0.0,
        }
    }
}

impl PitchConfig {
    /// LADSPA `pitch_shift` / `Pitch co-efficient` factor from semitones.
    pub fn factor(&self) -> f32 {
        2f32.powf(self.semitones / 12.0)
    }
}

/// Chorus — `multivoice_chorus_1201`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChorusConfig {
    pub enabled: bool,
    /// 1-8 voices.
    pub voices: u32,
    /// 10-40 ms.
    pub delay_ms: f32,
    /// 0-2 ms.
    pub sep_ms: f32,
    /// 0-5 %.
    pub detune_pct: f32,
    /// 2-30 Hz.
    pub lfo_hz: f32,
    /// -20-0 dB.
    pub atten_db: f32,
}

impl Default for ChorusConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            voices: 3,
            delay_ms: 20.0,
            sep_ms: 0.5,
            detune_pct: 1.0,
            lfo_hz: 4.0,
            atten_db: -3.0,
        }
    }
}

/// Delay — `delay_1898`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DelayConfig {
    pub enabled: bool,
    /// 0-5 s.
    pub delay_s: f32,
}

impl Default for DelayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            delay_s: 0.3,
        }
    }
}

impl DelayConfig {
    /// `Max Delay (s)` control — delay time plus headroom.
    pub fn max_delay_s(&self) -> f32 {
        self.delay_s + 0.5
    }
}

/// Distortion — `valve_1209`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DistortionConfig {
    pub enabled: bool,
    /// 0-1.
    pub level: f32,
    /// 0-1 (0 = warm/even harmonics, 1 = harsh/odd harmonics).
    pub character: f32,
}

impl Default for DistortionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            level: 0.3,
            character: 0.5,
        }
    }
}

/// Reverb — `gverb_1216`. Stereo output (`Left output` / `Right output`);
/// must be the last stage in the chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbConfig {
    pub enabled: bool,
    /// 1-300 m.
    pub roomsize_m: f32,
    /// 0.1-30 s.
    pub time_s: f32,
    /// 0-1.
    pub damping: f32,
    /// 0-1.
    pub bandwidth: f32,
    /// -70-0 dB.
    pub dry_db: f32,
    /// -70-0 dB.
    pub early_db: f32,
    /// -70-0 dB.
    pub tail_db: f32,
}

impl Default for ReverbConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roomsize_m: 30.0,
            time_s: 2.0,
            damping: 0.5,
            bandwidth: 0.75,
            dry_db: -3.0,
            early_db: -9.0,
            tail_db: -12.0,
        }
    }
}

/// Top-level VC LADSPA configuration persisted and sent over D-Bus as JSON.
///
/// Unlike NC (where every stage is always baked into the graph and neutralised
/// via bypass controls), these plugins have no true bypass port — chorus and
/// delay always colour the signal, even at minimal settings. So a disabled
/// effect is *omitted* from the generated graph entirely (same behaviour as
/// the Python `module-ladspa-source` chain), and the filter-chain process is
/// rebuilt whenever the set of enabled effects changes. See `vc_ladspa_chain.rs`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VcLadspaConfig {
    pub enabled: bool,
    /// Stable ALSA `node.name` of the physical mic source to process.
    pub source_id: String,
    pub pitch: PitchConfig,
    pub chorus: ChorusConfig,
    pub delay: DelayConfig,
    pub distortion: DistortionConfig,
    pub reverb: ReverbConfig,
}

impl VcLadspaConfig {
    pub fn active(&self) -> bool {
        self.enabled
    }
}

/// One model's calibrated tuning: the [`RvcParams`] dynamics knobs plus the
/// pitch shift that was found to fit that model's trained register (not
/// itself an `RvcParams` field — see `vc_calibration.rs`'s
/// `propose_pitch_variants` doc comment for why pitch is tuned separately
/// from dynamics). Keyed by model name in [`RvcConfig::model_params`], and
/// flattened to match the flat JSON shape the GUI already sends
/// (`vc_widget.py`'s `_apply`/`_current_model_params`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RvcModelSnapshot {
    pub pitch_offset: f32,
    #[serde(flatten)]
    pub params: RvcParams,
    /// Manual override for a model's native sample rate, used only when its
    /// `.onnx` predates `export_onnx.py` stamping it in — see
    /// `SynthSession::native_sample_rate`. Never set by the daemon itself;
    /// the GUI only offers this field once auto-detection has already come
    /// back empty ("extrema ratio", not a normal setting).
    pub sample_rate_override: Option<u32>,
}

/// RVC (neural voice conversion) settings — model selection, the pitch
/// pre-scan result, and the dynamics tuning. Direct port of the relevant
/// slice of the legacy Python `VCSettings` dataclass's `rvc_*` fields.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RvcConfig {
    /// Display name of the selected model (`RvcModel::name`).
    pub model: String,
    pub pitch_offset: f32,
    #[serde(flatten)]
    pub params: RvcParams,
    /// Per-model snapshots, so switching models restores its own tuning
    /// instead of carrying over whatever was last dialed in.
    pub model_params: HashMap<String, RvcModelSnapshot>,
}

/// Top-level voice-changer settings persisted and sent over D-Bus as JSON —
/// direct port of the legacy Python `VCSettings`'s `_to_dict()`/`load()`
/// shape (`enabled`/`mode`/`source_id`/`pitch`/`chorus`/`delay`/
/// `distortion`/`reverb` flattened at the top level, `rvc` nested), so the
/// existing GUI (`vc_widget.py`) needs no changes to talk to this daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VcSettings {
    /// `"ladspa"` | `"rvc"`.
    pub mode: String,
    #[serde(flatten)]
    pub ladspa: VcLadspaConfig,
    pub rvc: RvcConfig,
}

impl Default for VcSettings {
    fn default() -> Self {
        Self {
            mode: "ladspa".to_owned(),
            ladspa: VcLadspaConfig::default(),
            rvc: RvcConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_inactive() {
        let cfg = VcLadspaConfig::default();
        assert!(!cfg.active());
    }

    #[test]
    fn active_when_enabled() {
        let cfg = VcLadspaConfig {
            enabled: true,
            source_id: "alsa_input.usb-foo".to_owned(),
            ..Default::default()
        };
        assert!(cfg.active());
    }

    #[test]
    fn pitch_factor_zero_semitones_is_unity() {
        assert!((PitchConfig::default().factor() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pitch_factor_up_one_octave_is_double() {
        let cfg = PitchConfig {
            enabled: true,
            semitones: 12.0,
        };
        assert!((cfg.factor() - 2.0).abs() < 1e-4);
    }

    #[test]
    fn pitch_factor_down_one_octave_is_half() {
        let cfg = PitchConfig {
            enabled: true,
            semitones: -12.0,
        };
        assert!((cfg.factor() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn delay_max_delay_has_headroom_over_delay_time() {
        let cfg = DelayConfig {
            enabled: true,
            delay_s: 0.3,
        };
        assert!((cfg.max_delay_s() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn roundtrip_default() {
        let cfg = VcLadspaConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: VcLadspaConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn roundtrip_full_config() {
        let cfg = VcLadspaConfig {
            enabled: true,
            source_id: "alsa_input.usb-SteelSeries-00.mono-fallback".to_owned(),
            pitch: PitchConfig {
                enabled: true,
                semitones: -3.0,
            },
            chorus: ChorusConfig {
                enabled: true,
                voices: 5,
                delay_ms: 25.0,
                sep_ms: 1.0,
                detune_pct: 2.0,
                lfo_hz: 6.0,
                atten_db: -6.0,
            },
            delay: DelayConfig {
                enabled: true,
                delay_s: 0.5,
            },
            distortion: DistortionConfig {
                enabled: true,
                level: 0.6,
                character: 0.8,
            },
            reverb: ReverbConfig {
                enabled: true,
                roomsize_m: 50.0,
                time_s: 3.0,
                damping: 0.4,
                bandwidth: 0.6,
                dry_db: -1.0,
                early_db: -5.0,
                tail_db: -10.0,
            },
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: VcLadspaConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    // ── VcSettings / RvcConfig ───────────────────────────────────────────

    #[test]
    fn vc_settings_default_mode_is_ladspa() {
        assert_eq!(VcSettings::default().mode, "ladspa");
    }

    #[test]
    fn vc_settings_roundtrip_default() {
        let cfg = VcSettings::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: VcSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn vc_settings_json_shape_matches_legacy_python_top_level_keys() {
        // vc_widget.py's _apply() sends (and GetVCSettings must answer with)
        // enabled/mode/source_id/pitch/chorus/delay/distortion/reverb/rvc
        // all as top-level keys — the `ladspa` field's flatten must produce
        // that, not a nested "ladspa" object.
        let json = serde_json::to_string(&VcSettings::default()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object().unwrap();
        for key in [
            "enabled",
            "mode",
            "source_id",
            "pitch",
            "chorus",
            "delay",
            "distortion",
            "reverb",
            "rvc",
        ] {
            assert!(obj.contains_key(key), "missing top-level key {key:?}");
        }
        assert!(!obj.contains_key("ladspa"), "ladspa must not be nested");
    }

    #[test]
    fn rvc_config_model_params_shape_matches_legacy_python_flat_dict() {
        // vc_widget.py's model_params values are the same flat dict shape
        // as the top-level rvc params (pitch_offset + RvcParams fields),
        // not a further-nested {"params": {...}} object.
        let mut cfg = RvcConfig::default();
        cfg.model_params.insert(
            "MyVoice".to_owned(),
            RvcModelSnapshot {
                pitch_offset: 12.0,
                params: RvcParams {
                    target_rms: 0.1,
                    ..Default::default()
                },
                sample_rate_override: None,
            },
        );
        let json = serde_json::to_string(&cfg).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let snapshot = &value["model_params"]["MyVoice"];
        assert_eq!(snapshot["pitch_offset"], 12.0);
        assert_eq!(snapshot["target_rms"], 0.1);
        assert!(snapshot.get("params").is_none(), "params must be flattened");
    }

    #[test]
    fn rvc_config_roundtrip_with_model_params() {
        let mut cfg = RvcConfig {
            model: "MyVoice".to_owned(),
            pitch_offset: 12.0,
            ..Default::default()
        };
        cfg.model_params.insert(
            "MyVoice".to_owned(),
            RvcModelSnapshot {
                pitch_offset: 12.0,
                params: RvcParams::default(),
                sample_rate_override: Some(48000),
            },
        );
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RvcConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
