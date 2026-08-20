// D-Bus service: three interfaces matching the v2 bus API, all on bus name
// `name.giacomofurlan.ArctisManager.Next`.  State is shared with device tasks
// via `Arc<Mutex<AppState>>`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::state::SignalEvent as SE;
use device_config::codec::FieldValue;
use device_config::{DeviceConfig, FieldDef, FieldOrRef, FieldType, StructDef};
use serde_json::{Map, Value as JsonValue};
use tokio::sync::{broadcast, Mutex};
use tracing::{error, warn};
use zbus::object_server::SignalEmitter;
use zbus::{connection, interface};

use crate::audio::AudioSetup;
use crate::device_persistence;
use crate::eq_manager::{self as eq_manager, EqRuntime};
use crate::eq::preset::{list_presets, load_preset, preset_path, save_preset, EqPreset};
use crate::eq::settings::{load_eq_settings, save_eq_settings};
use crate::general_settings::GeneralSettings;
use crate::state::{AppState, DeviceCommand, SignalEvent};

// ── D-Bus constants ───────────────────────────────────────────────────────────

const BUS_NAME: &str = "name.giacomofurlan.ArctisManager.Next";
const STATUS_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Status";
const SETTINGS_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Settings";
const CONFIG_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Config";
const EQ_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/EQ";

// ── Status interface ──────────────────────────────────────────────────────────

struct StatusInterface {
    state: Arc<Mutex<AppState>>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Status")]
impl StatusInterface {
    async fn get_status(&self) -> String {
        let state = self.state.lock().await;
        build_status_json(&state)
    }

    #[zbus(signal)]
    async fn status_changed(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_connected(
        emitter: &SignalEmitter<'_>,
        product_id: u16,
        name: &str,
        capabilities: Vec<String>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_disconnected(emitter: &SignalEmitter<'_>, product_id: u16) -> zbus::Result<()>;
}

// ── Settings interface ────────────────────────────────────────────────────────

struct SettingsInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
    settings_base_dir: std::path::PathBuf,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Settings")]
impl SettingsInterface {
    async fn get_settings(&self) -> String {
        let state = self.state.lock().await;
        build_settings_json(&state)
    }

    async fn set_setting(&self, setting: &str, value: &str) -> bool {
        // General settings are persisted to disk and do not require a connected device.
        if GeneralSettings::is_general_field(setting) {
            let (ok, path) = {
                let mut state = self.state.lock().await;
                let ok = state.general_settings.set_field(setting, value);
                (ok, state.general_settings_path.clone())
            };
            if !ok {
                warn!("SetSetting: could not apply general field '{setting}' = '{value}'");
                return false;
            }
            if let Err(e) = {
                let state = self.state.lock().await;
                state.general_settings.save_to_file(&path)
            } {
                warn!("SetSetting: failed to persist general settings: {e}");
            }
            let json = {
                let s = self.state.lock().await;
                build_settings_json(&s)
            };
            let _ = self.signal_tx.send(SE::SettingsChanged { json });
            return true;
        }

        // Device settings require a connected device and a matching API.
        let (api_name, field_value) = {
            let state = self.state.lock().await;
            let Some(entry) = state.devices.values().next() else {
                return false;
            };
            let Some(api) = find_api_for_field(&entry.config, setting) else {
                warn!("SetSetting: no API writes field '{setting}'");
                return false;
            };
            let fv = match parse_setting_value(&entry.config, &api, setting, value) {
                Some(fv) => fv,
                None => {
                    warn!("SetSetting: could not parse value '{value}' for '{setting}'");
                    return false;
                }
            };
            (api, fv)
        };

        let sent = {
            let state = self.state.lock().await;
            let Some(entry) = state.devices.values().next() else {
                return false;
            };
            let mut values = HashMap::new();
            values.insert(setting.to_string(), field_value);
            entry
                .cmd_tx
                .send(DeviceCommand::WriteApi { api_name, values })
                .await
                .is_ok()
        };

        if sent {
            // Persist the written value: load existing overrides, update, save.
            let (vid, pid) = {
                let s = self.state.lock().await;
                if let Some(entry) = s.devices.values().next() {
                    (entry.vid, entry.pid)
                } else {
                    (0, 0)
                }
            };
            if vid != 0 || pid != 0 {
                let file_path =
                    device_persistence::settings_file_path(&self.settings_base_dir, vid, pid);
                let json_val: serde_json::Value =
                    serde_json::from_str(value).unwrap_or(serde_json::Value::Null);
                let mut overrides = device_persistence::load_device_settings(&file_path);
                overrides.insert(setting.to_string(), json_val);
                if let Err(e) = device_persistence::save_device_settings(&file_path, &overrides) {
                    warn!("SetSetting: failed to persist device settings: {e}");
                }
            }

            let json = {
                let s = self.state.lock().await;
                build_settings_json(&s)
            };
            let _ = self.signal_tx.send(SE::SettingsChanged { json });
        }

        sent
    }

    async fn get_version(&self) -> String {
        env!("LAM_VERSION").to_string()
    }

    async fn get_list_options(&self, list_name: &str) -> String {
        match list_name {
            "pulse_audio_devices" => {
                let sinks = crate::audio::list_audio_sinks().await;
                serde_json::to_string(&sinks).unwrap_or_else(|_| "[]".to_string())
            }
            _ => "[]".to_string(),
        }
    }

    #[zbus(signal)]
    async fn settings_changed(emitter: &SignalEmitter<'_>, settings_json: &str)
        -> zbus::Result<()>;
}

// ── EQ interface ──────────────────────────────────────────────────────────────

struct EqInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
    settings_base_dir: std::path::PathBuf,
    eq_runtime: Arc<Mutex<EqRuntime>>,
    audio_shared: Arc<Mutex<Option<AudioSetup>>>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.EQ")]
impl EqInterface {
    /// Returns EQ capabilities of the connected device.
    /// JSON: `{"has_hw_eq": bool, "hw_band_mode": "fixed_10"|"parametric_10"|"fixed_5"|null}`.
    async fn get_eq_capabilities(&self) -> String {
        // HW EQ capability is determined from device config.
        // Currently no device YAML defines EQ APIs, so always false.
        let _state = self.state.lock().await;
        serde_json::to_string(&serde_json::json!({
            "has_hw_eq": false,
            "hw_band_mode": null
        }))
        .unwrap_or_else(|_| r#"{"has_hw_eq":false,"hw_band_mode":null}"#.to_string())
    }

    /// Returns the full EQ settings JSON (both channels).
    async fn get_eq_settings(&self) -> String {
        let settings = load_eq_settings(&self.settings_base_dir);
        serde_json::to_string(&settings).unwrap_or_else(|_| "{}".to_string())
    }

    /// Set a single EQ field on a channel.
    ///
    /// `channel`: `"media"` or `"chat"`.
    /// `key`: `"enabled"`, `"backend"`, `"band_mode"`, or `"preset"`.
    /// `value`: JSON-encoded value.
    async fn set_eq_setting(&self, channel: &str, key: &str, value: &str) -> bool {
        if channel != "media" && channel != "chat" {
            return false;
        }
        let mut settings = load_eq_settings(&self.settings_base_dir);
        let ch = if channel == "media" { &mut settings.media } else { &mut settings.chat };

        let ok = match key {
            "enabled" => {
                if let Ok(b) = serde_json::from_str::<bool>(value) {
                    ch.enabled = b;
                    true
                } else {
                    false
                }
            }
            "backend" => {
                if let Ok(b) = serde_json::from_str(value) {
                    ch.backend = b;
                    true
                } else {
                    false
                }
            }
            "band_mode" => {
                if let Ok(m) = serde_json::from_str(value) {
                    ch.band_mode = m;
                    true
                } else {
                    false
                }
            }
            "preset" => {
                if let Ok(name) = serde_json::from_str::<String>(value) {
                    ch.preset = name;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };

        if !ok {
            return false;
        }

        if let Err(e) = save_eq_settings(&self.settings_base_dir, &settings) {
            warn!("SetEQSetting: failed to persist: {e}");
        }

        // Apply or disable EQ pipeline.
        let ch_settings = if channel == "media" { &settings.media } else { &settings.chat }.clone();
        if ch_settings.enabled {
            eq_manager::apply_channel_eq(
                &ch_settings,
                channel,
                &self.settings_base_dir,
                &self.audio_shared,
                &self.eq_runtime,
            )
            .await;
        } else {
            eq_manager::disable_channel_eq(channel, &self.audio_shared, &self.eq_runtime).await;
        }

        let json = serde_json::to_string(&settings).unwrap_or_default();
        let _ = self.signal_tx.send(SignalEvent::EQChanged { json });
        true
    }

    /// Returns a JSON array of preset summaries: `[{"name": "...", "band_mode": "..."}]`.
    async fn list_presets(&self) -> String {
        let presets = list_presets(&self.settings_base_dir);
        let summaries: Vec<serde_json::Value> = presets
            .iter()
            .map(|p| serde_json::json!({"name": p.name, "band_mode": p.band_mode}))
            .collect();
        serde_json::to_string(&summaries).unwrap_or_else(|_| "[]".to_string())
    }

    /// Returns the full preset JSON for `name`, or `{}` if not found.
    async fn get_preset(&self, name: &str) -> String {
        let path = preset_path(&self.settings_base_dir, name);
        match load_preset(&path) {
            Ok(p) => serde_json::to_string(&p).unwrap_or_else(|_| "{}".to_string()),
            Err(_) => "{}".to_string(),
        }
    }

    /// Save (or overwrite) a preset from JSON.  Returns `true` on success.
    async fn save_preset(&self, preset_json: &str) -> bool {
        let preset: EqPreset = match serde_json::from_str(preset_json) {
            Ok(p) => p,
            Err(e) => {
                warn!("SavePreset: invalid JSON: {e}");
                return false;
            }
        };
        match save_preset(&self.settings_base_dir, &preset) {
            Ok(()) => true,
            Err(e) => {
                warn!("SavePreset: save failed: {e}");
                false
            }
        }
    }

    /// Delete a preset by name.  Returns `true` if the file was removed.
    async fn delete_preset(&self, name: &str) -> bool {
        let path = preset_path(&self.settings_base_dir, name);
        match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) => {
                warn!("DeletePreset: {e}");
                false
            }
        }
    }

    /// Returns JSON array of currently active PulseAudio clients.
    /// Each entry: `{"name": "...", "pid": <u32>}`.
    async fn get_running_streams(&self) -> String {
        running_streams_json().await
    }

    /// Returns JSON array of installed Steam games.
    /// Each entry: `{"app_id": <u32>, "name": "..."}`.
    async fn get_steam_games(&self) -> String {
        steam_games_json()
    }

    #[zbus(signal)]
    async fn eq_changed(emitter: &SignalEmitter<'_>, settings_json: &str) -> zbus::Result<()>;
}

// ── EQ helpers ────────────────────────────────────────────────────────────────

async fn running_streams_json() -> String {
    let out = tokio::process::Command::new("pactl")
        .args(["-f", "json", "list", "clients"])
        .output()
        .await;
    let Ok(out) = out else { return "[]".to_string() };
    if !out.status.success() {
        return "[]".to_string();
    }
    let Ok(clients) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return "[]".to_string();
    };
    let Some(arr) = clients.as_array() else { return "[]".to_string() };
    let result: Vec<serde_json::Value> = arr
        .iter()
        .filter_map(|c| {
            let name = c["properties"]["application.name"].as_str()?.to_owned();
            // Skip PipeWire/PulseAudio internal clients.
            if name.starts_with("pipewire") || name.starts_with("PulseAudio") {
                return None;
            }
            let pid = c["properties"]["application.process.id"]
                .as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            Some(serde_json::json!({"name": name, "pid": pid}))
        })
        .collect();
    serde_json::to_string(&result).unwrap_or_else(|_| "[]".to_string())
}

fn steam_games_json() -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let Some(home) = home else { return "[]".to_string() };
    let steamapps = home.join(".steam/steam/steamapps");
    let Ok(entries) = std::fs::read_dir(&steamapps) else { return "[]".to_string() };
    let mut games: Vec<serde_json::Value> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "acf") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        // Parse minimal VDF: look for "appid" and "name" lines.
        let app_id: Option<u32> = content
            .lines()
            .find(|l| l.trim_start().starts_with(r#""appid""#))
            .and_then(|l| l.split('"').nth(3))
            .and_then(|s| s.parse().ok());
        let name: Option<&str> = content
            .lines()
            .find(|l| l.trim_start().starts_with(r#""name""#))
            .and_then(|l| l.split('"').nth(3));
        if let (Some(id), Some(n)) = (app_id, name) {
            games.push(serde_json::json!({"app_id": id, "name": n}));
        }
    }
    games.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    serde_json::to_string(&games).unwrap_or_else(|_| "[]".to_string())
}

// ── Config interface ──────────────────────────────────────────────────────────

struct ConfigInterface {
    state: Arc<Mutex<AppState>>,
    signal_tx: broadcast::Sender<SignalEvent>,
}

#[interface(name = "name.giacomofurlan.ArctisManager.Next.Config")]
impl ConfigInterface {
    async fn reload_configs(&self) -> bool {
        let dirs = {
            let state = self.state.lock().await;
            state.config_dirs.clone()
        };
        let dir_refs: Vec<&std::path::Path> =
            dirs.iter().map(std::path::PathBuf::as_path).collect();
        let new_configs = crate::load_configs_from_dirs(&dir_refs);
        let json = {
            let mut state = self.state.lock().await;
            state.configs = new_configs;
            build_settings_json(&state)
        };
        let _ = self.signal_tx.send(SE::SettingsChanged { json });
        true
    }
}

// ── Service startup ───────────────────────────────────────────────────────────

/// Register all interfaces on the session bus and spawn a background task that
/// forwards `SignalEvent`s from device tasks to D-Bus signals.
///
/// Returns the `Connection` so the caller can keep it alive.
pub async fn start_dbus_service(
    state: Arc<Mutex<AppState>>,
    mut signal_rx: broadcast::Receiver<SignalEvent>,
    signal_tx: broadcast::Sender<SignalEvent>,
    settings_base_dir: std::path::PathBuf,
    audio_shared: Arc<Mutex<Option<AudioSetup>>>,
) -> zbus::Result<zbus::Connection> {
    let eq_runtime = EqRuntime::new();

    let conn = connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(
            STATUS_PATH,
            StatusInterface {
                state: Arc::clone(&state),
            },
        )?
        .serve_at(
            SETTINGS_PATH,
            SettingsInterface {
                state: Arc::clone(&state),
                signal_tx: signal_tx.clone(),
                settings_base_dir: settings_base_dir.clone(),
            },
        )?
        .serve_at(
            CONFIG_PATH,
            ConfigInterface {
                state: Arc::clone(&state),
                signal_tx: signal_tx.clone(),
            },
        )?
        .serve_at(
            EQ_PATH,
            EqInterface {
                state: Arc::clone(&state),
                signal_tx: signal_tx.clone(),
                settings_base_dir,
                eq_runtime: Arc::clone(&eq_runtime),
                audio_shared,
            },
        )?
        .build()
        .await?;

    let status_emitter = SignalEmitter::new(&conn, STATUS_PATH)?.into_owned();
    let settings_emitter = SignalEmitter::new(&conn, SETTINGS_PATH)?.into_owned();
    let eq_emitter = SignalEmitter::new(&conn, EQ_PATH)?.into_owned();

    tokio::spawn(async move {
        loop {
            let event = match signal_rx.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("D-Bus signal emitter lagged, dropped {n} events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            match event {
                SignalEvent::StatusChanged => {
                    let json = {
                        let s = state.lock().await;
                        build_status_json(&s)
                    };
                    if let Err(e) = StatusInterface::status_changed(&status_emitter, &json).await {
                        error!("StatusChanged signal failed: {e}");
                    }
                }
                SignalEvent::SettingsChanged { json } => {
                    if let Err(e) =
                        SettingsInterface::settings_changed(&settings_emitter, &json).await
                    {
                        error!("SettingsChanged signal failed: {e}");
                    }
                }
                SignalEvent::EQChanged { json } => {
                    if let Err(e) = EqInterface::eq_changed(&eq_emitter, &json).await {
                        error!("EQChanged signal failed: {e}");
                    }
                }
                SignalEvent::DeviceConnected {
                    pid,
                    name,
                    capabilities,
                } => {
                    if let Err(e) =
                        StatusInterface::device_connected(&status_emitter, pid, &name, capabilities)
                            .await
                    {
                        error!("DeviceConnected signal failed: {e}");
                    }
                }
                SignalEvent::DeviceDisconnected { pid } => {
                    if let Err(e) = StatusInterface::device_disconnected(&status_emitter, pid).await
                    {
                        error!("DeviceDisconnected signal failed: {e}");
                    }
                }
            }
        }
    });

    Ok(conn)
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn build_status_json(state: &AppState) -> String {
    let Some(entry) = state.devices.values().next() else {
        return "{}".to_string();
    };

    if entry.status.is_empty() {
        return "{}".to_string();
    }

    if let Some(representation) = &entry.config.representation {
        let mut result: Map<String, JsonValue> = Map::new();
        for (category, field_names) in representation {
            let mut cat_map: Map<String, JsonValue> = Map::new();
            for field_name in field_names {
                if let Some(val) = entry.status.get(field_name) {
                    cat_map.insert(field_name.clone(), val.clone());
                }
            }
            if !cat_map.is_empty() {
                result.insert(category.clone(), JsonValue::Object(cat_map));
            }
        }
        if result.is_empty() {
            return "{}".to_string();
        }
        return serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());
    }

    // Fallback: group all fields under "headset" when no representation is defined.
    let fields: Map<String, JsonValue> = entry
        .status
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let result = serde_json::json!({"headset": fields});
    serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string())
}

fn build_settings_json(state: &AppState) -> String {
    // Start with the general section, always present regardless of device connection.
    let general_json = state.general_settings.to_json();
    let general_config = GeneralSettings::settings_config_json();

    let mut result = serde_json::json!({
        "general": general_json,
        "device": {},
        "settings_config": general_config
    });

    let Some(entry) = state.devices.values().next() else {
        return serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
    };

    let (Some(apis), Some(structs)) = (&entry.config.apis, &entry.config.structs) else {
        return serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
    };

    let mut device_map = Map::new();
    let config_map = result["settings_config"].as_object_mut().unwrap();

    for (api_name, api_def) in apis {
        if api_def.write.is_none() {
            continue;
        }
        let Some(struct_def) = structs.get(api_name.as_str()) else {
            continue;
        };
        for fdef in outgoing_fields(struct_def, structs) {
            if fdef.constant.is_some() {
                continue;
            }
            let current = entry
                .status
                .get(&fdef.name)
                .and_then(|v| v.get("value"))
                .cloned();

            // device section: plain current value (no {value, type} wrapper)
            if let Some(val) = &current {
                device_map
                    .entry(fdef.name.clone())
                    .or_insert_with(|| val.clone());
            }

            config_map
                .entry(fdef.name.clone())
                .or_insert_with(|| field_to_schema(fdef, current.as_ref()));
        }
    }

    result["device"] = JsonValue::Object(device_map);

    serde_json::to_string(&result).unwrap_or_else(|_| result.to_string())
}

fn yaml_to_json(y: &serde_yaml::Value) -> JsonValue {
    if let Some(i) = y.as_i64() {
        JsonValue::from(i)
    } else if let Some(f) = y.as_f64() {
        JsonValue::from(f)
    } else if let Some(s) = y.as_str() {
        JsonValue::from(s)
    } else if let Some(b) = y.as_bool() {
        JsonValue::from(b)
    } else {
        JsonValue::Null
    }
}

/// Build a `ConfigSetting`-compatible schema for one writable field.
///
/// The GUI constructs `ConfigSetting(name=..., **schema)`, so every key here
/// becomes an attribute.  Required keys: `type` (SettingType value string) and
/// `default_value`.
fn field_to_schema(fdef: &FieldDef, current_val: Option<&JsonValue>) -> JsonValue {
    // default_value: prefer the live current value, fall back to range minimum.
    let default_val = current_val.cloned().unwrap_or_else(|| {
        fdef.range
            .as_ref()
            .and_then(|r| r.first())
            .map(yaml_to_json)
            .unwrap_or(JsonValue::from(0i64))
    });

    if let Some(range) = &fdef.range {
        if range.len() == 2 {
            let min_f = range[0].as_f64().unwrap_or(0.0);
            let max_f = range[1].as_f64().unwrap_or(1.0);
            if (max_f - min_f) <= 1.0 {
                // Boolean [0, 1] range → toggle
                return serde_json::json!({
                    "type": "toggle",
                    "default_value": default_val,
                    "values": {
                        "on": yaml_to_json(&range[1]),
                        "off": yaml_to_json(&range[0]),
                        "on_label": "on",
                        "off_label": "off"
                    }
                });
            } else {
                // Wider numeric range → slider
                return serde_json::json!({
                    "type": "slider",
                    "default_value": default_val,
                    "min": yaml_to_json(&range[0]),
                    "max": yaml_to_json(&range[1]),
                    "step": 1
                });
            }
        }
    }

    if let Some(values) = &fdef.values {
        let mapping: Map<String, JsonValue> = values
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_str().map(|s| (i.to_string(), JsonValue::from(s))))
            .collect();
        return serde_json::json!({
            "type": "discrete_map",
            "default_value": default_val,
            "values_mapping": JsonValue::Object(mapping)
        });
    }

    // Fallback: treat as an unconstrained slider
    serde_json::json!({
        "type": "slider",
        "default_value": default_val,
        "min": 0,
        "max": 255,
        "step": 1
    })
}

/// Return the "outgoing" (write-side) flat list of concrete `FieldDef`s for
/// a struct, resolving one level of `{struct: name}` references.
fn outgoing_fields<'a>(
    struct_def: &'a StructDef,
    all_structs: &'a HashMap<String, StructDef>,
) -> Vec<&'a FieldDef> {
    let field_list: &[FieldOrRef] = match struct_def {
        StructDef::Bidir { outgoing, .. } => outgoing,
        StructDef::Flat(f) => f,
    };
    resolve_fields(field_list, all_structs)
}

fn resolve_fields<'a>(
    fields: &'a [FieldOrRef],
    all_structs: &'a HashMap<String, StructDef>,
) -> Vec<&'a FieldDef> {
    let mut result = Vec::new();
    for fof in fields {
        match fof {
            FieldOrRef::Field(fd) => result.push(fd),
            FieldOrRef::Ref { struct_ref } => {
                if let Some(inner) = all_structs.get(struct_ref.as_str()) {
                    // Resolve one level; avoid infinite recursion by not calling recursively.
                    let inner_fields = match inner {
                        StructDef::Bidir { outgoing, .. } => outgoing.as_slice(),
                        StructDef::Flat(f) => f.as_slice(),
                    };
                    for fof2 in inner_fields {
                        if let FieldOrRef::Field(fd) = fof2 {
                            result.push(fd);
                        }
                    }
                }
            }
        }
    }
    result
}

// ── SetSetting helpers ────────────────────────────────────────────────────────

/// Find the name of the first API whose write struct contains a non-constant
/// field named `field_name`.
pub(crate) fn find_api_for_field(config: &DeviceConfig, field_name: &str) -> Option<String> {
    let apis = config.apis.as_ref()?;
    let structs = config.structs.as_ref()?;

    for (api_name, api_def) in apis {
        if api_def.write.is_none() {
            continue;
        }
        let Some(struct_def) = structs.get(api_name.as_str()) else {
            continue;
        };
        let fields = outgoing_fields(struct_def, structs);
        for fdef in fields {
            if fdef.constant.is_none() && fdef.name == field_name {
                return Some(api_name.clone());
            }
        }
    }
    None
}

/// Parse a JSON-encoded string value into a `FieldValue` matching the type
/// expected by `field_name` in `api_name`'s write struct.
pub(crate) fn parse_setting_value(
    config: &DeviceConfig,
    api_name: &str,
    field_name: &str,
    raw: &str,
) -> Option<FieldValue> {
    let structs = config.structs.as_ref()?;
    let struct_def = structs.get(api_name)?;
    let fields = outgoing_fields(struct_def, structs);
    let fdef = fields.into_iter().find(|f| f.name == field_name)?;

    let json_val: serde_json::Value = serde_json::from_str(raw).ok()?;

    match fdef.field_type {
        FieldType::Uint8 => Some(FieldValue::U8(json_val.as_u64()? as u8)),
        FieldType::Uint16 => Some(FieldValue::U16(json_val.as_u64()? as u16)),
        FieldType::Uint32 => Some(FieldValue::U32(json_val.as_u64()? as u32)),
        FieldType::Float32 => Some(FieldValue::F32(json_val.as_f64()? as f32)),
        FieldType::ByteArray => None, // byte-array fields are not settable via D-Bus
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_version_is_semver_like() {
        let v = env!("LAM_VERSION");
        assert!(!v.is_empty());
        assert!(
            v.split('.').count() >= 2,
            "'{v}' has fewer than 2 version components"
        );
    }

    #[test]
    fn settings_changed_event_carries_json() {
        use tokio::sync::broadcast;
        let (tx, mut rx) = broadcast::channel::<SignalEvent>(4);
        let payload = r#"{"general":{},"device":{"volume":50},"settings_config":{}}"#;
        tx.send(SignalEvent::SettingsChanged {
            json: payload.to_string(),
        })
        .unwrap();
        match rx.try_recv().unwrap() {
            SignalEvent::SettingsChanged { json } => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap();
                assert_eq!(v["device"]["volume"], 50);
            }
            _ => panic!("wrong variant"),
        }
    }
    use device_config::{ApiDef, ApiOp, FieldType, Transport};
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    fn dummy_entry_with_status(
        status: HashMap<String, JsonValue>,
        cmd_tx: mpsc::Sender<DeviceCommand>,
    ) -> crate::state::DeviceEntry {
        let config = DeviceConfig::default();
        crate::state::DeviceEntry {
            config: Arc::new(config),
            vid: 0,
            pid: 0,
            name: "test".to_string(),
            capabilities: vec![],
            status,
            cmd_tx,
        }
    }

    #[test]
    fn build_status_json_empty() {
        let state = AppState {
            configs: vec![],
            devices: HashMap::new(),
            config_dirs: vec![PathBuf::from("/tmp")],
            general_settings: crate::general_settings::GeneralSettings::default(),
            general_settings_path: PathBuf::from("/tmp/gs.yaml"),
        };
        assert_eq!(build_status_json(&state), "{}");
    }

    #[test]
    fn build_status_json_with_field() {
        let (tx, _rx) = mpsc::channel(1);
        let mut status = HashMap::new();
        status.insert(
            "battery".to_string(),
            serde_json::json!({"value": 80, "type": "uint8"}),
        );
        let mut devices = HashMap::new();
        devices.insert(
            PathBuf::from("/dev/hidraw0"),
            dummy_entry_with_status(status, tx),
        );
        let state = AppState {
            configs: vec![],
            devices,
            config_dirs: vec![PathBuf::from("/tmp")],
            general_settings: crate::general_settings::GeneralSettings::default(),
            general_settings_path: PathBuf::from("/tmp/gs.yaml"),
        };
        let json: JsonValue = serde_json::from_str(&build_status_json(&state)).unwrap();
        // Fields are nested under the "headset" category for GUI compatibility.
        assert_eq!(json["headset"]["battery"]["value"], 80);
        assert_eq!(json["headset"]["battery"]["type"], "uint8");
    }

    #[test]
    fn build_status_json_uses_representation() {
        let (tx, _rx) = mpsc::channel(1);
        let mut status = HashMap::new();
        status.insert(
            "battery".to_string(),
            serde_json::json!({"value": 80, "type": "uint8"}),
        );
        status.insert(
            "wireless_mode".to_string(),
            serde_json::json!({"value": "speed", "type": "label"}),
        );
        let mut rep = HashMap::new();
        rep.insert("headset".to_string(), vec!["battery".to_string()]);
        rep.insert("wireless".to_string(), vec!["wireless_mode".to_string()]);
        let config = DeviceConfig {
            representation: Some(rep),
            ..DeviceConfig::default()
        };
        let mut devices = HashMap::new();
        devices.insert(
            std::path::PathBuf::from("/dev/hidraw0"),
            crate::state::DeviceEntry {
                config: Arc::new(config),
                vid: 0,
                pid: 0,
                name: "test".to_string(),
                capabilities: vec![],
                status,
                cmd_tx: tx,
            },
        );
        let state = AppState {
            configs: vec![],
            devices,
            config_dirs: vec![std::path::PathBuf::from("/tmp")],
            general_settings: crate::general_settings::GeneralSettings::default(),
            general_settings_path: std::path::PathBuf::from("/tmp/gs.yaml"),
        };
        let json: JsonValue = serde_json::from_str(&build_status_json(&state)).unwrap();
        assert_eq!(json["headset"]["battery"]["value"], 80);
        assert_eq!(json["wireless"]["wireless_mode"]["value"], "speed");
        // mic category was not in representation, so must be absent
        assert!(json.get("mic").is_none());
    }

    #[test]
    fn build_settings_json_general_section_populated() {
        use crate::general_settings::GeneralSettings;
        let state = AppState {
            configs: vec![],
            devices: HashMap::new(),
            config_dirs: vec![PathBuf::from("/tmp")],
            general_settings: GeneralSettings {
                redirect_audio_on_connect: true,
                redirect_audio_on_disconnect: false,
                redirect_audio_on_disconnect_device: Some("alsa_output.test".to_owned()),
            },
            general_settings_path: PathBuf::from("/tmp/gs.yaml"),
        };
        let json: JsonValue = serde_json::from_str(&build_settings_json(&state)).unwrap();
        assert_eq!(json["general"]["redirect_audio_on_connect"], true);
        assert_eq!(json["general"]["redirect_audio_on_disconnect"], false);
        assert_eq!(
            json["general"]["redirect_audio_on_disconnect_device"],
            "alsa_output.test"
        );
        // settings_config must include the 3 general fields
        assert_eq!(
            json["settings_config"]["redirect_audio_on_connect"]["type"],
            "toggle"
        );
        assert_eq!(
            json["settings_config"]["redirect_audio_on_disconnect_device"]["type"],
            "select"
        );
    }

    #[test]
    fn find_api_for_field_found() {
        use std::collections::HashMap as Map;

        let mut structs = Map::new();
        structs.insert(
            "set_vol".to_string(),
            StructDef::Flat(vec![FieldOrRef::Field(FieldDef {
                name: "volume".to_string(),
                field_type: FieldType::Uint8,
                constant: None,
                range: None,
                values: None,
                repeat: None,
                size: None,
            })]),
        );
        let mut apis = Map::new();
        apis.insert(
            "set_vol".to_string(),
            ApiDef {
                read: None,
                write: Some(ApiOp {
                    transport: Transport::HidIo,
                    chunk_size: 64,
                    payload_transform: None,
                }),
            },
        );
        let config = DeviceConfig {
            structs: Some(structs),
            apis: Some(apis),
            ..DeviceConfig::default()
        };

        assert_eq!(
            find_api_for_field(&config, "volume"),
            Some("set_vol".to_string())
        );
        assert_eq!(find_api_for_field(&config, "nonexistent"), None);
    }
}
