// Voice Changer LADSPA effect chain manager.
//
// Same mechanism as `nc_manager.rs`: spawns `pipewire -c <generated_conf>`
// with libpipewire-module-filter-chain, exposing `Arctis_VC_Mic` as an
// Audio/Source. Callers point `Arctis_Manager_Mic` at it via `mic_router`
// (VC output takes priority over NC — see `mic_router.rs`).
//
// Unlike NC, these plugins have no true bypass port: chorus and delay always
// colour the signal, even at minimal settings. So a disabled effect is
// *omitted* from the graph entirely (same behaviour as the Python
// `module-ladspa-source` chain) rather than baked-in-and-neutralised.
// The filter-chain process is rebuilt whenever the *set* of enabled effects
// changes; a pure parameter change on an unchanged set is a live control push.
//
// Not yet wired into dbus.rs — the `VcInterface` D-Bus service and
// `mic_router` hookup land in a later phase ([E10-S5], see
// docs/voice-changing-feature.md and docs/v3-backlog.md). Unit tests below
// exercise this module directly in the meantime.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use crate::ladspa_util::find_plugin;
use crate::vc_config::{
    ChorusConfig, DelayConfig, DistortionConfig, PitchConfig, ReverbConfig, VcLadspaConfig,
    CHORUS_CANDIDATES, DELAY_CANDIDATES, DISTORTION_CANDIDATES, PITCH_CANDIDATES,
    REVERB_CANDIDATES,
};

// ── Public constants ──────────────────────────────────────────────────────────

/// Audio/Source node exposed by the running filter-chain graph.
pub const VC_MIC: &str = "Arctis_VC_Mic";
/// Capture-side (invisible) stream node inside the graph.
pub const VC_INPUT: &str = "Arctis_VC_Mic_input";
const VC_MIC_DESC: &str = "Arctis Manager Voice Mic (internal)";

pub fn capabilities() -> std::collections::HashMap<&'static str, bool> {
    std::collections::HashMap::from([
        ("pitch", find_plugin(PITCH_CANDIDATES).is_some()),
        ("chorus", find_plugin(CHORUS_CANDIDATES).is_some()),
        ("delay", find_plugin(DELAY_CANDIDATES).is_some()),
        ("distortion", find_plugin(DISTORTION_CANDIDATES).is_some()),
        ("reverb", find_plugin(REVERB_CANDIDATES).is_some()),
    ])
}

// ── Stage descriptor ──────────────────────────────────────────────────────────
// Every VC stage is a LADSPA plugin with a single "Input" / "Output" audio
// port (verified via `analyseplugin` against the real swh-plugins binaries),
// except reverb (`gverb`, "Left output" / "Right output") — which is always
// last, so its output ports never need to appear in an inter-stage link; the
// filter-chain module auto-connects the last node's outputs to the graph's
// stereo playback.

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Stage {
    name: String,
    plugin: String,
    label: String,
    controls: Vec<(String, f64)>,
}

// ── Control helpers (pure — LADSPA port names verified via `analyseplugin`) ───

fn pitch_controls(cfg: &PitchConfig, plugin: &str) -> Vec<(String, f64)> {
    let factor = cfg.factor() as f64;
    if plugin.contains("am_pitchshift") {
        vec![
            ("Pitch shift".to_owned(), factor),
            ("Buffer size".to_owned(), 4.0),
        ]
    } else {
        vec![("Pitch co-efficient".to_owned(), factor)]
    }
}

fn chorus_controls(cfg: &ChorusConfig) -> Vec<(String, f64)> {
    vec![
        ("Number of voices".to_owned(), cfg.voices as f64),
        ("Delay base (ms)".to_owned(), cfg.delay_ms as f64),
        ("Voice separation (ms)".to_owned(), cfg.sep_ms as f64),
        ("Detune (%)".to_owned(), cfg.detune_pct as f64),
        ("LFO frequency (Hz)".to_owned(), cfg.lfo_hz as f64),
        ("Output attenuation (dB)".to_owned(), cfg.atten_db as f64),
    ]
}

fn delay_controls(cfg: &DelayConfig) -> Vec<(String, f64)> {
    vec![
        ("Max Delay (s)".to_owned(), cfg.max_delay_s() as f64),
        ("Delay Time (s)".to_owned(), cfg.delay_s as f64),
    ]
}

fn distortion_controls(cfg: &DistortionConfig) -> Vec<(String, f64)> {
    vec![
        ("Distortion level".to_owned(), cfg.level as f64),
        ("Distortion character".to_owned(), cfg.character as f64),
    ]
}

fn reverb_controls(cfg: &ReverbConfig) -> Vec<(String, f64)> {
    vec![
        ("Roomsize (m)".to_owned(), cfg.roomsize_m as f64),
        ("Reverb time (s)".to_owned(), cfg.time_s as f64),
        ("Damping".to_owned(), cfg.damping as f64),
        ("Input bandwidth".to_owned(), cfg.bandwidth as f64),
        ("Dry signal level (dB)".to_owned(), cfg.dry_db as f64),
        (
            "Early reflection level (dB)".to_owned(),
            cfg.early_db as f64,
        ),
        ("Tail level (dB)".to_owned(), cfg.tail_db as f64),
    ]
}

// ── Stage collection ──────────────────────────────────────────────────────────

/// Collect stages for the *currently enabled* effects, in a fixed order
/// (reverb last, per its stereo-output constraint). An effect that is enabled
/// but whose plugin is not installed is skipped with a warning — it never
/// silently no-ops.
pub(crate) fn collect_stages(config: &VcLadspaConfig) -> Vec<Stage> {
    let mut stages = Vec::new();

    if config.pitch.enabled {
        match find_plugin(PITCH_CANDIDATES) {
            Some((plugin, label)) => stages.push(Stage {
                name: "pitch".to_owned(),
                plugin: plugin.to_owned(),
                label: label.to_owned(),
                controls: pitch_controls(&config.pitch, plugin),
            }),
            None => warn!("VC: pitch requested but no pitch-shift plugin found — skipping"),
        }
    }

    if config.chorus.enabled {
        match find_plugin(CHORUS_CANDIDATES) {
            Some((plugin, label)) => stages.push(Stage {
                name: "chorus".to_owned(),
                plugin: plugin.to_owned(),
                label: label.to_owned(),
                controls: chorus_controls(&config.chorus),
            }),
            None => warn!("VC: chorus requested but multivoice_chorus not found — skipping"),
        }
    }

    if config.delay.enabled {
        match find_plugin(DELAY_CANDIDATES) {
            Some((plugin, label)) => stages.push(Stage {
                name: "delay".to_owned(),
                plugin: plugin.to_owned(),
                label: label.to_owned(),
                controls: delay_controls(&config.delay),
            }),
            None => warn!("VC: delay requested but delay_1898 not found — skipping"),
        }
    }

    if config.distortion.enabled {
        match find_plugin(DISTORTION_CANDIDATES) {
            Some((plugin, label)) => stages.push(Stage {
                name: "distortion".to_owned(),
                plugin: plugin.to_owned(),
                label: label.to_owned(),
                controls: distortion_controls(&config.distortion),
            }),
            None => warn!("VC: distortion requested but valve_1209 not found — skipping"),
        }
    }

    if config.reverb.enabled {
        match find_plugin(REVERB_CANDIDATES) {
            Some((plugin, label)) => stages.push(Stage {
                name: "reverb".to_owned(),
                plugin: plugin.to_owned(),
                label: label.to_owned(),
                controls: reverb_controls(&config.reverb),
            }),
            None => warn!("VC: reverb requested but gverb_1216 not found — skipping"),
        }
    }

    stages
}

// ── Config file generation ────────────────────────────────────────────────────

pub(crate) fn generate_conf(config: &VcLadspaConfig, stages: &[Stage]) -> String {
    let nodes: Vec<String> = stages
        .iter()
        .map(|s| {
            let controls = if s.controls.is_empty() {
                String::new()
            } else {
                let inner: String = s
                    .controls
                    .iter()
                    .map(|(port, val)| format!("                            \"{port}\" = {val}\n"))
                    .collect();
                format!("                        control = {{\n{inner}                        }}\n")
            };
            format!(
                "                    {{\n\
                                         type   = ladspa\n\
                                         name   = {}\n\
                                         plugin = {}\n\
                                         label  = {}\n\
                 {controls}\
                                     }}",
                s.name, s.plugin, s.label
            )
        })
        .collect();

    let links: Vec<String> = stages
        .windows(2)
        .map(|w| {
            format!(
                "                    {{ output = \"{}:Output\" input = \"{}:Input\" }}",
                w[0].name, w[1].name
            )
        })
        .collect();

    format!(
        r#"# Generated by Arctis Manager — voice-changer LADSPA filter chain.
context.properties = {{
    log.level = 2
}}

context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}

context.modules = [
    {{ name = libpipewire-module-rt
        args = {{ }}
        flags = [ ifexists nofail ]
    }}
    {{ name = libpipewire-module-protocol-native }}
    {{ name = libpipewire-module-client-node }}
    {{ name = libpipewire-module-adapter }}
    {{ name = libpipewire-module-filter-chain
        args = {{
            node.description = "{VC_MIC_DESC}"
            media.name       = "{VC_MIC_DESC}"
            filter.graph = {{
                nodes = [
{nodes}
                ]
                links = [
{links}
                ]
            }}
            audio.position = [ FL FR ]
            capture.props = {{
                node.name    = "{VC_INPUT}"
                node.passive = true
                node.always-process = true
                target.object = "{source_id}"
            }}
            playback.props = {{
                node.name    = "{VC_MIC}"
                media.class  = Audio/Source
                node.virtual = true
                device.class = "filter"
            }}
        }}
    }}
]
"#,
        nodes = nodes.join("\n"),
        links = links.join("\n"),
        source_id = config.source_id,
    )
}

fn conf_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(runtime_dir)
        .join("arctis-manager")
        .join("vc-ladspa-filter-chain.conf")
}

// ── VC LADSPA runtime ─────────────────────────────────────────────────────────

/// Live VC LADSPA state held in `Arc<Mutex<VcLadspaRuntime>>`.
#[derive(Debug, Default)]
pub struct VcLadspaRuntime {
    proc: Option<Child>,
    input_node_id: Option<u32>,
    /// source_id baked into the running graph.
    baked_source_id: String,
    /// Stage names baked into the running graph (used to detect topology changes).
    baked_stage_names: Vec<String>,
}

impl VcLadspaRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when the filter-chain process is running.
    pub fn is_active(&self) -> bool {
        self.proc
            .as_ref()
            .map(|p| p.id().is_some())
            .unwrap_or(false)
    }
}

// ── Public apply / teardown ───────────────────────────────────────────────────

/// Apply (or update) the VC LADSPA configuration.
/// Returns the name of the output source (`Arctis_VC_Mic`) on success.
pub async fn apply_vc_ladspa(
    config: &VcLadspaConfig,
    runtime: &mut VcLadspaRuntime,
) -> Option<&'static str> {
    if !config.active() {
        teardown_vc_ladspa(runtime).await;
        return None;
    }

    let source_id: String = if config.source_id.is_empty() {
        match crate::audio::find_physical_source().await {
            Some(s) => {
                info!("VC: auto-detected physical source: {s}");
                s
            }
            None => {
                error!("VC: no source_id configured and physical source not found");
                return None;
            }
        }
    } else {
        config.source_id.clone()
    };

    let stages = collect_stages(config);
    let stage_names: Vec<String> = stages.iter().map(|s| s.name.clone()).collect();

    // Try live update first (graph topology unchanged, only controls differ).
    if runtime.is_active()
        && runtime.baked_source_id == source_id
        && runtime.baked_stage_names == stage_names
    {
        if let Some(node_id) = runtime.input_node_id {
            if push_controls(node_id, &stages).await {
                info!("VC: controls updated live on node {node_id}");
                return Some(VC_MIC);
            }
            warn!("VC: live update failed — rebuilding graph");
        }
    }

    // Rebuild: stop existing process, write new conf, spawn.
    teardown_vc_ladspa(runtime).await;

    let effective_config;
    let conf_config = if config.source_id.is_empty() {
        effective_config = VcLadspaConfig {
            source_id: source_id.clone(),
            ..config.clone()
        };
        &effective_config
    } else {
        config
    };
    let conf = generate_conf(conf_config, &stages);
    let path = conf_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            error!("VC: cannot create conf dir {}: {e}", parent.display());
            return None;
        }
    }
    if let Err(e) = std::fs::write(&path, &conf) {
        error!("VC: cannot write conf to {}: {e}", path.display());
        return None;
    }

    let child = match spawn_pipewire(&path) {
        Ok(c) => c,
        Err(e) => {
            error!("VC: failed to spawn pipewire filter-chain: {e}");
            return None;
        }
    };
    runtime.proc = Some(child);

    let node_id = match wait_for_input_node(runtime, Duration::from_secs(3)).await {
        Some(id) => id,
        None => {
            error!("VC: filter-chain node '{VC_INPUT}' did not appear within 3 s");
            teardown_vc_ladspa(runtime).await;
            return None;
        }
    };

    runtime.input_node_id = Some(node_id);
    runtime.baked_source_id = source_id;
    runtime.baked_stage_names = stage_names;

    let chain: Vec<&str> = std::iter::once("<physical>")
        .chain(stages.iter().map(|s| s.name.as_str()))
        .chain(std::iter::once(VC_MIC))
        .collect();
    info!("VC: filter-chain active: {}", chain.join(" → "));
    Some(VC_MIC)
}

/// Stop the filter-chain process and reset runtime state.
pub async fn teardown_vc_ladspa(runtime: &mut VcLadspaRuntime) {
    if let Some(mut child) = runtime.proc.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        debug!("VC: filter-chain process stopped");
    }
    runtime.input_node_id = None;
    runtime.baked_source_id.clear();
    runtime.baked_stage_names.clear();
}

// ── Process management ────────────────────────────────────────────────────────

fn spawn_pipewire(conf: &Path) -> std::io::Result<Child> {
    Command::new("pipewire")
        .arg("-c")
        .arg(conf)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// Poll `pw-dump` until the `Arctis_VC_Mic_input` node appears, then return its ID.
async fn wait_for_input_node(runtime: &mut VcLadspaRuntime, timeout: Duration) -> Option<u32> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(ref mut child) = runtime.proc {
            match child.try_wait() {
                Ok(Some(status)) => {
                    error!("VC: filter-chain process exited early: {status}");
                    return None;
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        }

        if let Some(id) = poll_input_node().await {
            return Some(id);
        }

        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn poll_input_node() -> Option<u32> {
    let out = Command::new("pw-dump")
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())?;
    let json: Value = serde_json::from_slice(&out.stdout).ok()?;
    for obj in json.as_array()? {
        if obj.get("type")?.as_str() != Some("PipeWire:Interface:Node") {
            continue;
        }
        let props = obj.get("info")?.get("props")?;
        if props.get("node.name")?.as_str() == Some(VC_INPUT) {
            return obj.get("id")?.as_u64().map(|id| id as u32);
        }
    }
    None
}

/// Push updated controls to the running filter-chain node via `pw-cli`.
async fn push_controls(node_id: u32, stages: &[Stage]) -> bool {
    let pairs: Vec<String> = stages
        .iter()
        .flat_map(|s| {
            s.controls
                .iter()
                .map(move |(port, val)| format!("\"{}:{port}\", {val}", s.name))
        })
        .collect();

    if pairs.is_empty() {
        return true;
    }

    let spec = format!("{{ params = [ {} ] }}", pairs.join(", "));
    Command::new("pw-cli")
        .args(["s", &node_id.to_string(), "Props", &spec])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vc_config::VcLadspaConfig;

    fn minimal_config() -> VcLadspaConfig {
        VcLadspaConfig {
            enabled: true,
            source_id: "alsa_input.usb-foo.mono-fallback".to_owned(),
            ..Default::default()
        }
    }

    fn stage(name: &str, plugin: &str, label: &str, controls: Vec<(String, f64)>) -> Stage {
        Stage {
            name: name.to_owned(),
            plugin: plugin.to_owned(),
            label: label.to_owned(),
            controls,
        }
    }

    // ── generate_conf (pure string formatting, stage list built by hand) ───

    #[test]
    fn conf_contains_vc_mic_and_input_names() {
        let cfg = minimal_config();
        let stages = vec![stage("pitch", "am_pitchshift_1433", "amPitchshift", vec![])];
        let conf = generate_conf(&cfg, &stages);
        assert!(conf.contains(VC_MIC));
        assert!(conf.contains(VC_INPUT));
    }

    #[test]
    fn conf_contains_source_id() {
        let cfg = minimal_config();
        let stages = vec![stage("pitch", "am_pitchshift_1433", "amPitchshift", vec![])];
        let conf = generate_conf(&cfg, &stages);
        assert!(conf.contains(&cfg.source_id));
    }

    #[test]
    fn conf_links_consecutive_stages_output_to_input() {
        let cfg = minimal_config();
        let stages = vec![
            stage("pitch", "am_pitchshift_1433", "amPitchshift", vec![]),
            stage(
                "chorus",
                "multivoice_chorus_1201",
                "multivoiceChorus",
                vec![],
            ),
        ];
        let conf = generate_conf(&cfg, &stages);
        assert!(
            conf.contains("pitch:Output") && conf.contains("chorus:Input"),
            "pitch→chorus link not found in conf"
        );
    }

    #[test]
    fn conf_with_single_stage_has_no_links() {
        let cfg = minimal_config();
        let stages = vec![stage("reverb", "gverb_1216", "gverb", vec![])];
        let conf = generate_conf(&cfg, &stages);
        assert!(
            conf.contains("links = [\n\n"),
            "expected an empty links array"
        );
    }

    #[test]
    fn conf_with_no_stages_has_no_nodes_or_links() {
        let cfg = minimal_config();
        let conf = generate_conf(&cfg, &[]);
        assert!(conf.contains("nodes = [\n\n"));
        assert!(conf.contains("links = [\n\n"));
    }

    #[test]
    fn conf_contains_plugin_and_control_values() {
        let cfg = minimal_config();
        let stages = vec![stage(
            "distortion",
            "valve_1209",
            "valve",
            vec![
                ("Distortion level".to_owned(), 0.6),
                ("Distortion character".to_owned(), 0.8),
            ],
        )];
        let conf = generate_conf(&cfg, &stages);
        assert!(conf.contains("valve_1209"));
        assert!(conf.contains("\"Distortion level\" = 0.6"));
        assert!(conf.contains("\"Distortion character\" = 0.8"));
    }

    // ── Control builders (pure, exact LADSPA port names) ────────────────────

    #[test]
    fn pitch_controls_use_am_pitchshift_ports_when_selected() {
        let cfg = PitchConfig {
            enabled: true,
            semitones: 12.0,
        };
        let controls = pitch_controls(&cfg, "am_pitchshift_1433");
        assert_eq!(controls[0].0, "Pitch shift");
        assert!((controls[0].1 - 2.0).abs() < 1e-4);
        assert_eq!(controls[1], ("Buffer size".to_owned(), 4.0));
    }

    #[test]
    fn pitch_controls_use_pitch_scale_single_port_when_fallback() {
        let cfg = PitchConfig {
            enabled: true,
            semitones: 0.0,
        };
        let controls = pitch_controls(&cfg, "pitch_scale_1193");
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].0, "Pitch co-efficient");
        assert!((controls[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn delay_controls_max_delay_has_headroom() {
        let cfg = DelayConfig {
            enabled: true,
            delay_s: 1.0,
        };
        let controls = delay_controls(&cfg);
        assert_eq!(controls[0], ("Max Delay (s)".to_owned(), 1.5));
        assert_eq!(controls[1], ("Delay Time (s)".to_owned(), 1.0));
    }

    #[test]
    fn reverb_controls_map_all_seven_ports() {
        let cfg = ReverbConfig {
            enabled: true,
            ..Default::default()
        };
        let controls = reverb_controls(&cfg);
        assert_eq!(controls.len(), 7);
        assert_eq!(controls[0].0, "Roomsize (m)");
        assert_eq!(controls[6].0, "Tail level (dB)");
    }

    // ── collect_stages ordering (availability-independent parts) ───────────

    #[test]
    fn collect_stages_returns_empty_when_all_disabled() {
        let cfg = VcLadspaConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(collect_stages(&cfg).is_empty());
    }
}
