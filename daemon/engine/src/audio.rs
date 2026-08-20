// Virtual PipeWire sink management via pactl.
//
// Creates Arctis_Media and Arctis_Chat null-sinks with loopbacks to the
// physical Arctis headset output.  All operations are async (tokio process).

use std::fmt;

use serde::Serialize;
use tokio::process::Command;
use tracing::{info, warn};

const MEDIA_SINK: &str = "Arctis_Media";
const CHAT_SINK: &str = "Arctis_Chat";
const STEELSERIES_VID: &str = "0x1038";

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AudioError {
    Pactl(String),
    PhysicalSinkNotFound,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Pactl(msg) => write!(f, "pactl: {msg}"),
            AudioError::PhysicalSinkNotFound => write!(f, "physical Arctis sink not found"),
        }
    }
}

// ── Audio sink enumeration ────────────────────────────────────────────────────

/// A user-visible PipeWire/PulseAudio output sink.
#[derive(Debug, Clone, Serialize)]
pub struct AudioSink {
    /// `node.name` — stable ALSA path, rename-proof.
    pub id: String,
    /// `node.nick` — human-readable display label.
    pub name: String,
}

/// Parse `pactl -f json list sinks` output into `AudioSink` pairs.
/// Skips the virtual sinks created by this daemon (`Arctis_Media`, `Arctis_Chat`).
/// This is a pure function so it can be unit-tested without a running PipeWire.
pub fn parse_audio_sinks(json: &str) -> Vec<AudioSink> {
    let Ok(sinks) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    let Some(arr) = sinks.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|sink| {
            let id = sink["name"].as_str()?;
            if id == MEDIA_SINK || id == CHAT_SINK {
                return None;
            }
            let nick = sink["properties"]["node.nick"].as_str().unwrap_or(id);
            Some(AudioSink {
                id: id.to_owned(),
                name: nick.to_owned(),
            })
        })
        .collect()
}

/// Enumerate all non-virtual PipeWire output sinks.
/// Returns an empty list if `pactl` is unavailable or returns an error.
pub async fn list_audio_sinks() -> Vec<AudioSink> {
    match pactl(&["-f", "json", "list", "sinks"]).await {
        Ok(json) => parse_audio_sinks(&json),
        Err(e) => {
            warn!("audio: list sinks failed: {e}");
            vec![]
        }
    }
}

// ── Module tracking ───────────────────────────────────────────────────────────

/// PulseAudio module indices loaded for one device session.
/// Used to unload exactly what we created on device disconnect.
#[derive(Debug, Clone)]
pub struct AudioSetup {
    media_null: u32,
    chat_null: u32,
    media_loopback: u32,
    chat_loopback: u32,
}

// ── pactl helpers ─────────────────────────────────────────────────────────────

async fn pactl(args: &[&str]) -> Result<String, AudioError> {
    let out = Command::new("pactl")
        .args(args)
        .output()
        .await
        .map_err(|e| AudioError::Pactl(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(AudioError::Pactl(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Find the first non-virtual Arctis sink (vendor 0x1038) and return its name.
/// Retries a few times because the audio device may appear slightly after the
/// HID interface.
pub async fn find_physical_sink() -> Option<String> {
    for attempt in 0..5u8 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        if let Some(name) = try_find_physical_sink().await {
            return Some(name);
        }
    }
    None
}

async fn try_find_physical_sink() -> Option<String> {
    let json = pactl(&["-f", "json", "list", "sinks"]).await.ok()?;
    let sinks: serde_json::Value = serde_json::from_str(&json).ok()?;
    for sink in sinks.as_array()? {
        let name = sink["name"].as_str().unwrap_or("");
        if name == MEDIA_SINK || name == CHAT_SINK {
            continue;
        }
        let vid = sink["properties"]["device.vendor.id"]
            .as_str()
            .unwrap_or("");
        if vid == STEELSERIES_VID {
            return Some(name.to_owned());
        }
    }
    None
}

/// Load a PulseAudio module and return its index, or None on failure.
async fn load_module(module: &str, args: &str) -> Option<u32> {
    match pactl(&["load-module", module, args]).await {
        Ok(out) => out.parse::<u32>().ok(),
        Err(e) => {
            warn!("load-module {module} failed: {e}");
            None
        }
    }
}

/// Parse `pactl list short modules` into (index, module_name, args) tuples.
/// PipeWire omits the `index` field from JSON module output, so we use the
/// short tab-separated format which always carries the index as column 0.
async fn list_short_modules() -> Vec<(u32, String, String)> {
    let Ok(out) = pactl(&["list", "short", "modules"]).await else {
        return vec![];
    };
    out.lines()
        .filter_map(|line| {
            let mut cols = line.splitn(3, '\t');
            let idx: u32 = cols.next()?.trim().parse().ok()?;
            let name = cols.next()?.trim().to_string();
            let args = cols.next().unwrap_or("").trim().to_string();
            Some((idx, name, args))
        })
        .collect()
}

fn find_in_modules(mods: &[(u32, String, String)], module_name: &str, arg: &str) -> Option<u32> {
    mods.iter()
        .find(|(_, n, a)| n == module_name && a.contains(arg))
        .map(|(i, _, _)| *i)
}

/// Check whether a null-sink with `name` and its monitor loopback already exist.
async fn sink_and_loopback_exist(name: &str, physical: &str) -> bool {
    let mods = list_short_modules().await;
    let lb_source = format!("source={name}.monitor");
    let lb_sink = format!("sink={physical}");
    let has_null =
        find_in_modules(&mods, "module-null-sink", &format!("sink_name={name}")).is_some();
    let has_loopback = mods
        .iter()
        .any(|(_, n, a)| n == "module-loopback" && a.contains(&lb_source) && a.contains(&lb_sink));
    has_null && has_loopback
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Create Arctis_Media and Arctis_Chat virtual sinks with loopbacks to the
/// physical Arctis headset output.  Idempotent: skips creation if the sink
/// and its loopback already exist from a previous session.
pub async fn setup_sinks() -> Result<AudioSetup, AudioError> {
    let physical = find_physical_sink()
        .await
        .ok_or(AudioError::PhysicalSinkNotFound)?;
    info!("audio: physical Arctis sink = {physical}");

    let (media_null, media_loopback) = ensure_sink(MEDIA_SINK, "Arctis Media", &physical).await?;
    let (chat_null, chat_loopback) = ensure_sink(CHAT_SINK, "Arctis Chat", &physical).await?;

    Ok(AudioSetup {
        media_null,
        chat_null,
        media_loopback,
        chat_loopback,
    })
}

async fn ensure_sink(
    name: &str,
    description: &str,
    physical: &str,
) -> Result<(u32, u32), AudioError> {
    if sink_and_loopback_exist(name, physical).await {
        // Sinks survive daemon restarts; retrieve existing module indices.
        info!("audio: {name} already set up, reusing");
        return get_module_indices(name, physical).await;
    }

    // Outer quotes let the module-arg parser treat the entire proplist as one
    // token; inner single-quotes quote the description value within it.
    let null_args =
        format!("sink_name={name} sink_properties=\"node.description='{description}'\"");
    let null_idx = load_module("module-null-sink", &null_args)
        .await
        .ok_or_else(|| AudioError::Pactl(format!("module-null-sink for {name}")))?;
    info!("audio: created {name} (module {null_idx})");

    let lb_args = format!("source={name}.monitor sink={physical} latency_msec=0");
    let lb_idx = load_module("module-loopback", &lb_args)
        .await
        .ok_or_else(|| AudioError::Pactl(format!("module-loopback for {name}")))?;
    info!("audio: loopback {name}.monitor -> {physical} (module {lb_idx})");

    Ok((null_idx, lb_idx))
}

/// Retrieve module indices for sinks that already existed before this session.
async fn get_module_indices(name: &str, physical: &str) -> Result<(u32, u32), AudioError> {
    let mods = list_short_modules().await;
    let null_idx = find_in_modules(&mods, "module-null-sink", &format!("sink_name={name}"))
        .ok_or_else(|| AudioError::Pactl(format!("cannot find null-sink index for {name}")))?;
    let lb_source = format!("source={name}.monitor");
    let lb_sink = format!("sink={physical}");
    let lb_idx = mods
        .iter()
        .find(|(_, n, a)| n == "module-loopback" && a.contains(&lb_source) && a.contains(&lb_sink))
        .map(|(i, _, _)| *i)
        .ok_or_else(|| AudioError::Pactl(format!("cannot find loopback index for {name}")))?;
    Ok((null_idx, lb_idx))
}

/// Update the game/chat volume split on the virtual sinks.
/// Values are 0–100.
pub async fn set_chatmix(game: u8, chat: u8) {
    let game_pct = format!("{game}%");
    let chat_pct = format!("{chat}%");

    if let Err(e) = pactl(&["set-sink-volume", MEDIA_SINK, &game_pct]).await {
        warn!("audio: set-sink-volume {MEDIA_SINK}: {e}");
    }
    if let Err(e) = pactl(&["set-sink-volume", CHAT_SINK, &chat_pct]).await {
        warn!("audio: set-sink-volume {CHAT_SINK}: {e}");
    }
}

/// Unload all modules created by `setup_sinks`.
pub async fn teardown_sinks(setup: AudioSetup) {
    for id in [
        setup.media_loopback,
        setup.chat_loopback,
        setup.media_null,
        setup.chat_null,
    ] {
        let id_str = id.to_string();
        if let Err(e) = pactl(&["unload-module", &id_str]).await {
            warn!("audio: unload-module {id}: {e}");
        }
    }
    info!("audio: virtual sinks removed");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_error_display() {
        assert!(!AudioError::PhysicalSinkNotFound.to_string().is_empty());
        assert!(!AudioError::Pactl("oops".into()).to_string().is_empty());
    }

    #[test]
    fn parse_audio_sinks_skips_virtual_and_extracts_fields() {
        let json = r#"[
            {"name": "Arctis_Media",  "properties": {"node.nick": "Arctis Media"}},
            {"name": "Arctis_Chat",   "properties": {"node.nick": "Arctis Chat"}},
            {"name": "alsa_output.usb-SteelSeries-00", "properties": {"node.nick": "Arctis Nova"}}
        ]"#;
        let sinks = parse_audio_sinks(json);
        assert_eq!(sinks.len(), 1);
        assert_eq!(sinks[0].id, "alsa_output.usb-SteelSeries-00");
        assert_eq!(sinks[0].name, "Arctis Nova");
    }

    #[test]
    fn parse_audio_sinks_falls_back_to_id_when_nick_missing() {
        let json = r#"[{"name": "alsa_output.usb-foo", "properties": {}}]"#;
        let sinks = parse_audio_sinks(json);
        assert_eq!(sinks.len(), 1);
        assert_eq!(sinks[0].name, "alsa_output.usb-foo");
    }

    #[test]
    fn parse_audio_sinks_returns_empty_on_bad_json() {
        assert!(parse_audio_sinks("not json").is_empty());
        assert!(parse_audio_sinks("null").is_empty());
    }
}
