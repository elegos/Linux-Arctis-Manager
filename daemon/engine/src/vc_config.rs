// Voice Changer settings — LADSPA effect chain configuration.
//
// Field set and defaults are a direct port of the Python reference
// (`voice_changer/settings.py` / `voice_changer/ladspa/effects.py`). The RVC
// (neural voice conversion) config is a separate, later addition — see
// docs/voice-changing-feature.md.

use serde::{Deserialize, Serialize};

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
}
