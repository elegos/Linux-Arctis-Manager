// Noise Cancelling filter-chain manager.
//
// Preferred path: spawns `pipewire -c <generated_conf>` with
// libpipewire-module-filter-chain.  The graph exposes `Arctis_NC_Mic` as an
// Audio/Source.  Callers then point `Arctis_Manager_Mic` at it via MicRouter.
//
// All stage plugins found on the system are baked into the graph at startup;
// disabling a stage neutralises it via bypass controls so no graph rebuild is
// needed — controls are pushed live via `pw-cli s <node_id> Props`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use crate::ladspa_util::{find_plugin, plugin_available};
use crate::nc_config::{
    CompressorConfig, GateConfig, NcConfig, COMP_CANDIDATES, GATE_CANDIDATES, RNNOISE_CONTROLS,
    RNNOISE_LABEL, RNNOISE_PLUGIN, RNNOISE_PLUGIN_ALT,
};

// ── Public constants ──────────────────────────────────────────────────────────

/// Audio/Source node exposed by the running filter-chain graph.
pub const NC_MIC: &str = "Arctis_NC_Mic";
/// Capture-side (invisible) stream node inside the graph.
pub const NC_INPUT: &str = "Arctis_NC_Mic_input";
const NC_MIC_DESC: &str = "Arctis Manager NC Mic (internal)";

/// True when the gate and compressor (sc4m) plugins are both present.
pub fn swh_available() -> bool {
    find_plugin(GATE_CANDIDATES).is_some() && find_plugin(COMP_CANDIDATES).is_some()
}

pub fn rnnoise_plugin() -> Option<&'static str> {
    if plugin_available(RNNOISE_PLUGIN) {
        Some(RNNOISE_PLUGIN)
    } else if plugin_available(RNNOISE_PLUGIN_ALT) {
        Some(RNNOISE_PLUGIN_ALT)
    } else {
        None
    }
}

// ── Stage descriptor ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum StageType {
    Builtin,
    Ladspa { plugin: String },
}

#[derive(Debug, Clone)]
pub(crate) struct Stage {
    name: String,
    stage_type: StageType,
    label: String,
    controls: Vec<(String, f64)>,
}

impl Stage {
    fn port_in(&self) -> &str {
        match &self.stage_type {
            StageType::Builtin => "In",
            StageType::Ladspa { .. } => "Input",
        }
    }
    fn port_out(&self) -> &str {
        match &self.stage_type {
            StageType::Builtin => "Out",
            StageType::Ladspa { .. } => "Output",
        }
    }
}

// ── Control helpers ───────────────────────────────────────────────────────────

fn hpf_controls(enabled: bool) -> Vec<(String, f64)> {
    vec![
        ("Freq".to_owned(), if enabled { 90.0 } else { 10.0 }),
        ("Q".to_owned(), 0.707),
    ]
}

fn gate_controls(cfg: &GateConfig) -> Vec<(String, f64)> {
    vec![
        ("LF key filter (Hz)".to_owned(), 150.0),
        ("HF key filter (Hz)".to_owned(), 4000.0),
        ("Threshold (dB)".to_owned(), cfg.threshold as f64),
        ("Attack (ms)".to_owned(), cfg.attack as f64),
        ("Hold (ms)".to_owned(), 2.0),
        ("Decay (ms)".to_owned(), cfg.release as f64),
        ("Range (dB)".to_owned(), cfg.reduction.max(-90) as f64),
        (
            "Output select (-1 = key listen, 0 = gate, 1 = bypass)".to_owned(),
            if cfg.enabled { 0.0 } else { 1.0 },
        ),
    ]
}

fn comp_controls(cfg: &CompressorConfig) -> Vec<(String, f64)> {
    vec![
        ("RMS/peak".to_owned(), 0.0),
        ("Attack time (ms)".to_owned(), 20.0),
        ("Release time (ms)".to_owned(), 150.0),
        (
            "Threshold level (dB)".to_owned(),
            cfg.threshold.max(-30) as f64,
        ),
        (
            "Ratio (1:n)".to_owned(),
            if cfg.enabled {
                cfg.ratio as f64 / 10.0
            } else {
                1.0
            },
        ),
        ("Knee radius (dB)".to_owned(), 1.0),
        (
            "Makeup gain (dB)".to_owned(),
            if cfg.enabled { cfg.makeup as f64 } else { 0.0 },
        ),
    ]
}

// ── Stage collection ──────────────────────────────────────────────────────────

/// Collect all stages available on this system for `config`.
/// Every found stage is included regardless of its `enabled` flag;
/// disabled stages use bypass/neutral controls so graph shape never changes.
pub(crate) fn collect_stages(config: &NcConfig, rnnoise: &str) -> Vec<Stage> {
    let mut stages = Vec::new();

    stages.push(Stage {
        name: "hpf".to_owned(),
        stage_type: StageType::Builtin,
        label: "bq_highpass".to_owned(),
        controls: hpf_controls(config.hpf_enabled),
    });

    let (vad, grace, retro) = RNNOISE_CONTROLS;
    stages.push(Stage {
        name: "rnnoise".to_owned(),
        stage_type: StageType::Ladspa {
            plugin: rnnoise.to_owned(),
        },
        label: RNNOISE_LABEL.to_owned(),
        controls: vec![
            ("VAD Threshold (%)".to_owned(), vad),
            ("VAD Grace Period (ms)".to_owned(), grace),
            ("Retroactive VAD Grace (ms)".to_owned(), retro),
        ],
    });

    if let Some((plugin, label)) = find_plugin(GATE_CANDIDATES) {
        stages.push(Stage {
            name: "gate".to_owned(),
            stage_type: StageType::Ladspa {
                plugin: plugin.to_owned(),
            },
            label: label.to_owned(),
            controls: gate_controls(&config.gate),
        });
    } else if config.gate.enabled {
        warn!("NC: gate requested but no gate plugin found — skipping");
    }

    if let Some((plugin, label)) = find_plugin(COMP_CANDIDATES) {
        stages.push(Stage {
            name: "comp".to_owned(),
            stage_type: StageType::Ladspa {
                plugin: plugin.to_owned(),
            },
            label: label.to_owned(),
            controls: comp_controls(&config.compressor),
        });
    } else if config.compressor.enabled {
        warn!("NC: compressor (sc4m) not found — skipping");
    }

    stages
}

// ── Config file generation ────────────────────────────────────────────────────

pub(crate) fn generate_conf(config: &NcConfig, stages: &[Stage]) -> String {
    let nodes: Vec<String> = stages
        .iter()
        .map(|s| {
            let type_str = match &s.stage_type {
                StageType::Builtin => "builtin".to_owned(),
                StageType::Ladspa { .. } => "ladspa".to_owned(),
            };
            let plugin_line = match &s.stage_type {
                StageType::Builtin => String::new(),
                StageType::Ladspa { plugin } => {
                    format!("                        plugin = {plugin}\n")
                }
            };
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
                                         type   = {type_str}\n\
                                         name   = {}\n\
                 {plugin_line}\
                                         label  = {}\n\
                 {controls}\
                                     }}",
                s.name, s.label
            )
        })
        .collect();

    let links: Vec<String> = stages
        .windows(2)
        .map(|w| {
            format!(
                "                    {{ output = \"{}:{}\" input = \"{}:{}\" }}",
                w[0].name,
                w[0].port_out(),
                w[1].name,
                w[1].port_in()
            )
        })
        .collect();

    format!(
        r#"# Generated by Arctis Manager — noise-cancellation filter chain.
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
            node.description = "{NC_MIC_DESC}"
            media.name       = "{NC_MIC_DESC}"
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
                node.name    = "{NC_INPUT}"
                node.passive = true
                node.always-process = true
                target.object = "{source_id}"
            }}
            playback.props = {{
                node.name    = "{NC_MIC}"
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
        .join("nc-filter-chain.conf")
}

// ── NC runtime ────────────────────────────────────────────────────────────────

/// Live NC state held in `Arc<Mutex<NcRuntime>>`.
#[derive(Debug, Default)]
pub struct NcRuntime {
    proc: Option<Child>,
    input_node_id: Option<u32>,
    /// source_id baked into the running graph.
    baked_source_id: String,
    /// Stage names baked into the running graph (used to detect topology changes).
    baked_stage_names: Vec<String>,
}

impl NcRuntime {
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

/// Apply (or update) the NC configuration.
/// Returns the name of the output source (`Arctis_NC_Mic`) on success.
pub async fn apply_nc(config: &NcConfig, runtime: &mut NcRuntime) -> Option<&'static str> {
    if !config.active() {
        teardown_nc(runtime).await;
        return None;
    }

    let source_id: String = if config.source_id.is_empty() {
        match crate::audio::find_physical_source().await {
            Some(s) => {
                info!("NC: auto-detected physical source: {s}");
                s
            }
            None => {
                error!("NC: no source_id configured and physical source not found");
                return None;
            }
        }
    } else {
        config.source_id.clone()
    };

    let rnnoise = match rnnoise_plugin() {
        Some(p) => p,
        None => {
            error!("NC: RNNoise LADSPA plugin not found (tried {RNNOISE_PLUGIN}.so, {RNNOISE_PLUGIN_ALT}.so)");
            return None;
        }
    };

    let stages = collect_stages(config, rnnoise);
    let stage_names: Vec<String> = stages.iter().map(|s| s.name.clone()).collect();

    // Try live update first (graph topology unchanged, only controls differ).
    if runtime.is_active()
        && runtime.baked_source_id == source_id
        && runtime.baked_stage_names == stage_names
    {
        if let Some(node_id) = runtime.input_node_id {
            if push_controls(node_id, &stages).await {
                info!("NC: controls updated live on node {node_id}");
                return Some(NC_MIC);
            }
            warn!("NC: live update failed — rebuilding graph");
        }
    }

    // Rebuild: stop existing process, write new conf, spawn.
    teardown_nc(runtime).await;

    // generate_conf uses config.source_id; substitute with resolved source_id.
    let effective_config;
    let conf_config = if config.source_id.is_empty() {
        effective_config = NcConfig {
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
            error!("NC: cannot create conf dir {}: {e}", parent.display());
            return None;
        }
    }
    if let Err(e) = std::fs::write(&path, &conf) {
        error!("NC: cannot write conf to {}: {e}", path.display());
        return None;
    }

    let child = match spawn_pipewire(&path) {
        Ok(c) => c,
        Err(e) => {
            error!("NC: failed to spawn pipewire filter-chain: {e}");
            return None;
        }
    };
    runtime.proc = Some(child);

    let node_id = match wait_for_input_node(runtime, Duration::from_secs(3)).await {
        Some(id) => id,
        None => {
            error!("NC: filter-chain node '{NC_INPUT}' did not appear within 3 s");
            teardown_nc(runtime).await;
            return None;
        }
    };

    runtime.input_node_id = Some(node_id);
    runtime.baked_source_id = source_id;
    runtime.baked_stage_names = stage_names;

    let chain: Vec<&str> = std::iter::once("<physical>")
        .chain(stages.iter().map(|s| s.name.as_str()))
        .chain(std::iter::once(NC_MIC))
        .collect();
    info!("NC: filter-chain active: {}", chain.join(" → "));
    Some(NC_MIC)
}

/// Stop the filter-chain process and reset runtime state.
pub async fn teardown_nc(runtime: &mut NcRuntime) {
    if let Some(mut child) = runtime.proc.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        debug!("NC: filter-chain process stopped");
    }
    runtime.input_node_id = None;
    runtime.baked_source_id.clear();
    runtime.baked_stage_names.clear();
}

// ── Process management ────────────────────────────────────────────────────────

fn spawn_pipewire(conf: &Path) -> std::io::Result<Child> {
    // kill_on_drop ensures the subprocess is killed if the daemon exits without
    // an explicit teardown call (e.g. panic or OOM kill).
    Command::new("pipewire")
        .arg("-c")
        .arg(conf)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// Poll `pw-dump` until the `Arctis_NC_Mic_input` node appears, then return its ID.
async fn wait_for_input_node(runtime: &mut NcRuntime, timeout: Duration) -> Option<u32> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // Check if the process already exited.
        if let Some(ref mut child) = runtime.proc {
            match child.try_wait() {
                Ok(Some(status)) => {
                    error!("NC: filter-chain process exited early: {status}");
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
        if props.get("node.name")?.as_str() == Some(NC_INPUT) {
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
    use crate::nc_config::{CompressorConfig, GateConfig, NcConfig};

    fn minimal_config() -> NcConfig {
        NcConfig {
            preset: "on".to_owned(),
            source_id: "alsa_input.usb-foo.mono-fallback".to_owned(),
            ..Default::default()
        }
    }

    // Build stages without relying on the filesystem — inject rnnoise directly.
    fn test_stages(config: &NcConfig) -> Vec<Stage> {
        let mut stages = Vec::new();
        stages.push(Stage {
            name: "hpf".to_owned(),
            stage_type: StageType::Builtin,
            label: "bq_highpass".to_owned(),
            controls: hpf_controls(config.hpf_enabled),
        });
        let (vad, grace, retro) = RNNOISE_CONTROLS;
        stages.push(Stage {
            name: "rnnoise".to_owned(),
            stage_type: StageType::Ladspa {
                plugin: RNNOISE_PLUGIN.to_owned(),
            },
            label: RNNOISE_LABEL.to_owned(),
            controls: vec![
                ("VAD Threshold (%)".to_owned(), vad),
                ("VAD Grace Period (ms)".to_owned(), grace),
                ("Retroactive VAD Grace (ms)".to_owned(), retro),
            ],
        });
        stages
    }

    #[test]
    fn conf_contains_nc_mic_and_input_names() {
        let cfg = minimal_config();
        let stages = test_stages(&cfg);
        let conf = generate_conf(&cfg, &stages);
        assert!(conf.contains(NC_MIC), "missing NC_MIC node name");
        assert!(conf.contains(NC_INPUT), "missing NC_INPUT capture node");
    }

    #[test]
    fn conf_contains_source_id() {
        let cfg = minimal_config();
        let stages = test_stages(&cfg);
        let conf = generate_conf(&cfg, &stages);
        assert!(
            conf.contains(&cfg.source_id),
            "source_id not in generated conf"
        );
    }

    #[test]
    fn conf_contains_rnnoise_plugin_and_label() {
        let cfg = minimal_config();
        let stages = test_stages(&cfg);
        let conf = generate_conf(&cfg, &stages);
        assert!(conf.contains(RNNOISE_PLUGIN));
        assert!(conf.contains(RNNOISE_LABEL));
    }

    #[test]
    fn conf_contains_hpf_builtin() {
        let cfg = minimal_config();
        let stages = test_stages(&cfg);
        let conf = generate_conf(&cfg, &stages);
        assert!(conf.contains("bq_highpass"));
        assert!(conf.contains("type   = builtin"));
    }

    #[test]
    fn conf_links_hpf_to_rnnoise() {
        let cfg = minimal_config();
        let stages = test_stages(&cfg);
        let conf = generate_conf(&cfg, &stages);
        // hpf builtin: Out port; rnnoise ladspa: Input port
        assert!(
            conf.contains("hpf:Out") && conf.contains("rnnoise:Input"),
            "hpf→rnnoise link not found in conf"
        );
    }

    #[test]
    fn hpf_disabled_uses_low_freq() {
        let controls = hpf_controls(false);
        let freq = controls.iter().find(|(k, _)| k == "Freq").unwrap().1;
        assert_eq!(freq, 10.0, "disabled HPF must use 10 Hz (sub-audible)");
    }

    #[test]
    fn hpf_enabled_uses_voice_freq() {
        let controls = hpf_controls(true);
        let freq = controls.iter().find(|(k, _)| k == "Freq").unwrap().1;
        assert_eq!(freq, 90.0);
    }

    #[test]
    fn gate_disabled_sets_bypass_port() {
        let gate = GateConfig {
            enabled: false,
            ..Default::default()
        };
        let controls = gate_controls(&gate);
        let output_select = controls
            .iter()
            .find(|(k, _)| k.contains("output select") || k.contains("Output select"))
            .unwrap()
            .1;
        assert_eq!(
            output_select, 1.0,
            "disabled gate must bypass (output select = 1)"
        );
    }

    #[test]
    fn gate_enabled_sets_gate_port() {
        let gate = GateConfig {
            enabled: true,
            ..Default::default()
        };
        let controls = gate_controls(&gate);
        let output_select = controls
            .iter()
            .find(|(k, _)| k.contains("output select") || k.contains("Output select"))
            .unwrap()
            .1;
        assert_eq!(output_select, 0.0);
    }

    #[test]
    fn comp_disabled_is_unity() {
        let comp = CompressorConfig {
            enabled: false,
            ..Default::default()
        };
        let controls = comp_controls(&comp);
        let ratio = controls
            .iter()
            .find(|(k, _)| k.contains("Ratio"))
            .unwrap()
            .1;
        let makeup = controls
            .iter()
            .find(|(k, _)| k.contains("Makeup"))
            .unwrap()
            .1;
        assert_eq!(ratio, 1.0, "disabled comp must have ratio 1:1");
        assert_eq!(makeup, 0.0, "disabled comp must have 0 dB makeup");
    }

    #[test]
    fn comp_enabled_uses_config_values() {
        let comp = CompressorConfig {
            enabled: true,
            threshold: -20,
            ratio: 30, // 3.0:1
            makeup: 6,
        };
        let controls = comp_controls(&comp);
        let ratio = controls
            .iter()
            .find(|(k, _)| k.contains("Ratio"))
            .unwrap()
            .1;
        assert!((ratio - 3.0).abs() < 0.01);
    }
}
