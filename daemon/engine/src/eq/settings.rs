use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::preset::BandMode;

// ── Backend selection ─────────────────────────────────────────────────────────

/// Which EQ processing backend to use for a channel.
///
/// LADSPA (`mbeq_1197`) is always available regardless of device capability;
/// it supports all three band modes as a complete, independent pipeline.
/// `hardware` sends commands directly to the headset via HID, which is faster
/// and requires no PipeWire module.  `auto` chooses hardware when the device
/// supports the active `band_mode`, otherwise falls back to LADSPA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EqBackend {
    /// Hardware HID EQ when supported; otherwise LADSPA.
    #[default]
    Auto,
    /// Always use the LADSPA `mbeq_1197` software pipeline.
    Ladspa,
    /// Always send HID EQ commands; silent LADSPA fallback when unsupported.
    Hardware,
}

// ── App override ──────────────────────────────────────────────────────────────

/// How an app-level EQ override matches the active audio stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AppMatcher {
    /// Match by the PipeWire application name registered by the client.
    Stream { name: String },
    /// Match by the full executable path of the audio-producing process.
    Executable { path: String },
    /// Match by the numeric Steam AppID.
    SteamGame { app_id: u32 },
}

/// An EQ preset pinned to a specific application or game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppOverride {
    pub matcher: AppMatcher,
    /// Name of the preset to activate (must exist in `eq_presets/`).
    pub preset: String,
    /// Backend override for this app rule; `None` inherits the channel default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<EqBackend>,
}

// ── Per-channel settings ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelEqSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub backend: EqBackend,
    #[serde(default = "default_band_mode")]
    pub band_mode: BandMode,
    /// Name of the active preset (must exist in `eq_presets/`).
    pub preset: String,
    #[serde(default)]
    pub app_overrides: Vec<AppOverride>,
}

fn default_band_mode() -> BandMode {
    BandMode::Fixed10
}

impl Default for ChannelEqSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: EqBackend::Auto,
            band_mode: BandMode::Fixed10,
            preset: "Flat".to_owned(),
            app_overrides: vec![],
        }
    }
}

// ── Full EQ settings ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EqSettings {
    #[serde(default)]
    pub media: ChannelEqSettings,
    #[serde(default)]
    pub chat: ChannelEqSettings,
}

// ── Persistence ───────────────────────────────────────────────────────────────

pub fn eq_settings_path(base_dir: &Path) -> PathBuf {
    base_dir.join("eq_settings.yaml")
}

pub fn save_eq_settings(base_dir: &Path, settings: &EqSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(base_dir)?;
    let yaml = serde_yaml::to_string(settings).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(eq_settings_path(base_dir), yaml)
}

pub fn load_eq_settings(base_dir: &Path) -> EqSettings {
    let path = eq_settings_path(base_dir);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return EqSettings::default();
    };
    serde_yaml::from_str(&content).unwrap_or_default()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── Defaults ──────────────────────────────────────────────────────────────

    #[test]
    fn channel_defaults_are_disabled_auto_fixed10_flat() {
        let ch = ChannelEqSettings::default();
        assert!(!ch.enabled);
        assert_eq!(ch.backend, EqBackend::Auto);
        assert_eq!(ch.band_mode, BandMode::Fixed10);
        assert_eq!(ch.preset, "Flat");
        assert!(ch.app_overrides.is_empty());
    }

    #[test]
    fn eq_settings_default_both_channels_default() {
        let s = EqSettings::default();
        assert_eq!(s.media, ChannelEqSettings::default());
        assert_eq!(s.chat, ChannelEqSettings::default());
    }

    // ── Serialisation ─────────────────────────────────────────────────────────

    #[test]
    fn eq_backend_serialises_snake_case() {
        let yaml = serde_yaml::to_string(&EqBackend::Auto).unwrap();
        assert!(yaml.contains("auto"), "got: {yaml}");
        let yaml = serde_yaml::to_string(&EqBackend::Ladspa).unwrap();
        assert!(yaml.contains("ladspa"), "got: {yaml}");
        let yaml = serde_yaml::to_string(&EqBackend::Hardware).unwrap();
        assert!(yaml.contains("hardware"), "got: {yaml}");
    }

    #[test]
    fn app_matcher_stream_roundtrip() {
        let m = AppMatcher::Stream {
            name: "Spotify".into(),
        };
        let yaml = serde_yaml::to_string(&m).unwrap();
        let back: AppMatcher = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn app_matcher_executable_roundtrip() {
        let m = AppMatcher::Executable {
            path: "/usr/bin/spotify".into(),
        };
        let yaml = serde_yaml::to_string(&m).unwrap();
        let back: AppMatcher = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn app_matcher_steam_roundtrip() {
        let m = AppMatcher::SteamGame { app_id: 730 };
        let yaml = serde_yaml::to_string(&m).unwrap();
        let back: AppMatcher = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn app_override_backend_optional_skipped_when_none() {
        let o = AppOverride {
            matcher: AppMatcher::SteamGame { app_id: 570 },
            preset: "Gaming".into(),
            backend: None,
        };
        let yaml = serde_yaml::to_string(&o).unwrap();
        assert!(
            !yaml.contains("backend"),
            "backend should be omitted: {yaml}"
        );
    }

    #[test]
    fn channel_settings_roundtrip_with_overrides() {
        let ch = ChannelEqSettings {
            enabled: true,
            backend: EqBackend::Ladspa,
            band_mode: BandMode::Parametric10,
            preset: "Studio".into(),
            app_overrides: vec![
                AppOverride {
                    matcher: AppMatcher::Stream {
                        name: "Firefox".into(),
                    },
                    preset: "Flat".into(),
                    backend: Some(EqBackend::Hardware),
                },
                AppOverride {
                    matcher: AppMatcher::SteamGame { app_id: 440 },
                    preset: "Gaming".into(),
                    backend: None,
                },
            ],
        };
        let yaml = serde_yaml::to_string(&ch).unwrap();
        let back: ChannelEqSettings = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, ch);
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let mut s = EqSettings::default();
        s.media.enabled = true;
        s.media.backend = EqBackend::Hardware;
        s.media.band_mode = BandMode::Fixed5;
        s.media.preset = "Arctis5Preset".into();
        s.chat.enabled = false;
        save_eq_settings(dir.path(), &s).unwrap();
        let loaded = load_eq_settings(dir.path());
        assert_eq!(loaded, s);
    }

    #[test]
    fn load_returns_default_when_file_absent() {
        let dir = tempdir().unwrap();
        let s = load_eq_settings(dir.path());
        assert_eq!(s, EqSettings::default());
    }

    #[test]
    fn load_returns_default_on_corrupt_yaml() {
        let dir = tempdir().unwrap();
        std::fs::write(eq_settings_path(dir.path()), b": bad yaml [").unwrap();
        let s = load_eq_settings(dir.path());
        assert_eq!(s, EqSettings::default());
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        save_eq_settings(&nested, &EqSettings::default()).unwrap();
        assert!(eq_settings_path(&nested).exists());
    }
}
