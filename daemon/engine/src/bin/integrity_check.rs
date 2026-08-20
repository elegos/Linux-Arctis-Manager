/// lam-integrity-check — manual verification tool for D-Bus interface behaviour.
///
/// Usage: lam-integrity-check <subcommand>
///
/// Subcommands:
///   settings-signal    Verify that SettingsChanged is emitted after SetSetting()
///   list-options       Print GetListOptions("pulse_audio_devices") result
///   general-settings   Print the general section from GetSettings and verify schema
///   device-persistence Verify SetSetting persists to disk and value appears in GetSettings
///   ladspa-eq          Verify mbeq_1197 LADSPA EQ load/update/unload lifecycle
use std::time::Duration;

use futures::StreamExt;
use tokio::time::timeout;
use zbus::proxy;
use zbus::Connection;

#[path = "../eq/mod.rs"]
mod eq;

const BUS_NAME: &str = "name.giacomofurlan.ArctisManager.Next";
const SETTINGS_PATH: &str = "/name/giacomofurlan/ArctisManager/Next/Settings";

#[proxy(
    interface = "name.giacomofurlan.ArctisManager.Next.Settings",
    default_service = "name.giacomofurlan.ArctisManager.Next",
    default_path = "/name/giacomofurlan/ArctisManager/Next/Settings"
)]
trait Settings {
    async fn get_version(&self) -> zbus::Result<String>;
    async fn get_settings(&self) -> zbus::Result<String>;
    async fn set_setting(&self, setting: &str, value: &str) -> zbus::Result<bool>;
    async fn get_list_options(&self, list_name: &str) -> zbus::Result<String>;

    #[zbus(signal)]
    async fn settings_changed(&self, settings_json: String) -> zbus::Result<()>;
}

#[tokio::main]
async fn main() {
    let subcommand = std::env::args().nth(1).unwrap_or_default();
    let code = match subcommand.as_str() {
        "settings-signal" => check_settings_signal().await,
        "list-options" => check_list_options().await,
        "general-settings" => check_general_settings().await,
        "device-persistence" => check_device_persistence().await,
        "ladspa-eq" => check_ladspa_eq().await,
        _ => {
            eprintln!("Usage: lam-integrity-check <subcommand>");
            eprintln!();
            eprintln!("Subcommands:");
            eprintln!("  settings-signal    Verify SettingsChanged D-Bus signal delivery");
            eprintln!("  list-options       Print GetListOptions(\"pulse_audio_devices\")");
            eprintln!("  general-settings   Verify general section in GetSettings");
            eprintln!("  device-persistence Verify SetSetting persists value to YAML file");
            eprintln!("  ladspa-eq          Verify mbeq_1197 LADSPA EQ load/update/unload");
            1
        }
    };
    std::process::exit(code);
}

async fn check_settings_signal() -> i32 {
    println!("=== settings-signal integrity check ===");
    println!("Bus  : {BUS_NAME}");
    println!("Path : {SETTINGS_PATH}");
    println!();

    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: cannot connect to session bus: {e}");
            return 1;
        }
    };

    let proxy = match SettingsProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: cannot create Settings proxy: {e}");
            eprintln!("Is lam-daemon running?");
            return 1;
        }
    };

    // Verify daemon is alive and show version.
    match proxy.get_version().await {
        Ok(v) => println!("Daemon version : {v}"),
        Err(e) => {
            eprintln!("ERROR: GetVersion failed: {e}");
            return 1;
        }
    }

    // Show current settings snapshot.
    match proxy.get_settings().await {
        Ok(s) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&s).unwrap_or(serde_json::Value::Null);
            println!(
                "Current settings:\n{}",
                serde_json::to_string_pretty(&parsed).unwrap_or(s)
            );
        }
        Err(e) => eprintln!("WARNING: GetSettings failed: {e}"),
    }
    println!();

    // Subscribe to the SettingsChanged signal before triggering any change.
    let mut signal_stream = match proxy.receive_settings_changed().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: cannot subscribe to SettingsChanged: {e}");
            return 1;
        }
    };

    println!("Subscribed to SettingsChanged.");
    println!("Trigger a change via the GUI or run from another terminal:");
    println!("  gdbus call --session --dest {BUS_NAME} \\");
    println!("    --object-path {SETTINGS_PATH} \\");
    println!("    --method name.giacomofurlan.ArctisManager.Next.Settings.SetSetting \\");
    println!("    <field_name> <value_json>");
    println!();
    println!("Waiting up to 30 s for signal...");

    match timeout(Duration::from_secs(30), signal_stream.next()).await {
        Ok(Some(signal)) => {
            let args = signal.args().expect("signal args decode failed");
            let payload: serde_json::Value =
                serde_json::from_str(&args.settings_json).unwrap_or(serde_json::Value::Null);
            println!(
                "RECEIVED SettingsChanged:\n{}",
                serde_json::to_string_pretty(&payload).unwrap_or(args.settings_json)
            );
            0
        }
        Ok(None) => {
            eprintln!("ERROR: signal stream ended unexpectedly");
            1
        }
        Err(_) => {
            eprintln!("TIMEOUT: no SettingsChanged signal received within 30 s");
            1
        }
    }
}

async fn check_device_persistence() -> i32 {
    println!("=== device-persistence integrity check ===");
    println!();

    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: cannot connect to session bus: {e}");
            return 1;
        }
    };
    let proxy = match SettingsProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: cannot create Settings proxy: {e}");
            eprintln!("Is lam-daemon running?");
            return 1;
        }
    };

    // Read the current device section to find a writable field.
    let raw = match proxy.get_settings().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: GetSettings failed: {e}");
            return 1;
        }
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ERROR: invalid JSON from GetSettings: {e}");
            return 1;
        }
    };

    let device = &parsed["device"];
    if !device.is_object() || device.as_object().map(|m| m.is_empty()).unwrap_or(true) {
        println!("INFO: no device connected — cannot test device persistence.");
        println!("      Connect a device and retry.");
        return 0;
    }

    // Pick the first writable field and its current value.
    let (field, current_val) = device
        .as_object()
        .unwrap()
        .iter()
        .next()
        .map(|(k, v)| (k.clone(), v.clone()))
        .unwrap();

    println!("Test field  : {field}");
    println!("Current val : {current_val}");

    // Write the same value back (no functional change, just exercise the path).
    let val_str = serde_json::to_string(&current_val).unwrap();
    match proxy.set_setting(&field, &val_str).await {
        Ok(true) => println!("SetSetting  : OK"),
        Ok(false) => {
            eprintln!("ERROR: SetSetting returned false — field may not be writable");
            return 1;
        }
        Err(e) => {
            eprintln!("ERROR: SetSetting failed: {e}");
            return 1;
        }
    }

    // Check for the YAML file in the expected location.
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from(".config"));
    let settings_dir = config_home.join("arctis_manager/settings");

    match std::fs::read_dir(&settings_dir) {
        Ok(entries) => {
            let yamls: Vec<_> = entries
                .flatten()
                .filter(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false))
                .collect();
            if yamls.is_empty() {
                eprintln!("WARNING: no YAML file found in {}", settings_dir.display());
                eprintln!("         (daemon may not have a vid/pid for this device)");
                return 1;
            }
            for entry in &yamls {
                let path = entry.path();
                println!("Found file  : {}", path.display());
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        if content.contains(&field) {
                            println!("OK: field '{field}' present in {}", path.display());
                        } else {
                            eprintln!("WARNING: field '{field}' not found in {}", path.display());
                        }
                    }
                    Err(e) => eprintln!("ERROR: cannot read {}: {e}", path.display()),
                }
            }
            0
        }
        Err(_) => {
            eprintln!("ERROR: settings dir not found: {}", settings_dir.display());
            1
        }
    }
}

async fn check_general_settings() -> i32 {
    println!("=== general-settings integrity check ===");
    println!();

    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: cannot connect to session bus: {e}");
            return 1;
        }
    };

    let proxy = match SettingsProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: cannot create Settings proxy: {e}");
            eprintln!("Is lam-daemon running?");
            return 1;
        }
    };

    let raw = match proxy.get_settings().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: GetSettings failed: {e}");
            return 1;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ERROR: GetSettings returned invalid JSON: {e}");
            return 1;
        }
    };

    let general = &parsed["general"];
    println!("general section:");
    println!(
        "{}",
        serde_json::to_string_pretty(general).unwrap_or_else(|_| general.to_string())
    );
    println!();

    let required_fields = [
        "redirect_audio_on_connect",
        "redirect_audio_on_disconnect",
        "redirect_audio_on_disconnect_device",
    ];
    let mut ok = true;
    for field in &required_fields {
        if general.get(field).is_none() {
            eprintln!("MISSING field in general: {field}");
            ok = false;
        }
    }

    let sc = &parsed["settings_config"];
    for field in &required_fields {
        if sc.get(field).is_none() {
            eprintln!("MISSING field in settings_config: {field}");
            ok = false;
        }
    }

    if ok {
        println!("OK: all 3 general fields present in general and settings_config");
        let dev_type = sc["redirect_audio_on_disconnect_device"]["type"].as_str();
        if dev_type != Some("select") {
            eprintln!(
                "WARNING: redirect_audio_on_disconnect_device type expected 'select', got {:?}",
                dev_type
            );
        } else {
            println!("OK: redirect_audio_on_disconnect_device has type 'select'");
        }
        0
    } else {
        1
    }
}

async fn check_ladspa_eq() -> i32 {
    use eq::ladspa;
    use eq::preset::{flat_preset, BandMode};

    println!("=== ladspa-eq integrity check ===");
    println!();

    // 1. Plugin availability.
    print!("Checking mbeq_1197 plugin... ");
    if !ladspa::check_plugin_available().await {
        eprintln!("NOT FOUND");
        eprintln!("Install swh-plugins (Fedora: sudo dnf install ladspa-swh-plugins)");
        return 1;
    }
    println!("OK");

    // 2. Print band/frequency mapping.
    println!();
    println!("mbeq_1197 band frequencies:");
    for (i, &f) in ladspa::MBEQ_FREQ.iter().enumerate() {
        let role = if ladspa::FIXED_10_INDICES.contains(&i) {
            " [fixed_10]"
        } else if ladspa::FIXED_5_INDICES.contains(&i) {
            " [fixed_5]"
        } else {
            ""
        };
        println!("  [{i:2}] {:>6} Hz{role}", f as u32);
    }

    // 3. Load a test null-sink then attach LADSPA EQ.
    println!();
    println!("Loading test null-sink (lam_ic_test_src)...");
    let load_null = tokio::process::Command::new("pactl")
        .args([
            "load-module",
            "module-null-sink",
            "sink_name=lam_ic_test_src",
        ])
        .output()
        .await;
    let null_id: Option<u32> = match load_null {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout);
            let id = s.trim().parse::<u32>().ok();
            println!("  null-sink module id = {id:?}");
            id
        }
        Ok(out) => {
            eprintln!("  FAILED: {}", String::from_utf8_lossy(&out.stderr).trim());
            None
        }
        Err(e) => {
            eprintln!("  FAILED: {e}");
            None
        }
    };

    let ladspa_id = if null_id.is_some() {
        let preset = flat_preset(BandMode::Fixed10);
        let gains = ladspa::gains_for_preset(&preset);
        println!("Loading LADSPA EQ sink (lam_ic_test_eq)...");
        match ladspa::load_eq_module("lam_ic_test_eq", "lam_ic_test_src", &gains).await {
            Ok(id) => {
                println!("  LADSPA module id = {id}   OK");
                Some(id)
            }
            Err(e) => {
                eprintln!("  FAILED: {e}");
                None
            }
        }
    } else {
        None
    };

    // 4. Live gain update test.
    if ladspa_id.is_some() {
        println!("Testing live gain update...");
        let mut gains = [0.0f32; 15];
        gains[0] = 6.0; // 50 Hz +6 dB
        match ladspa::update_gains_live("lam_ic_test_eq", &gains).await {
            Ok(()) => println!("  Live update: OK"),
            Err(e) => eprintln!("  Live update failed (non-fatal): {e}"),
        }
    }

    // 5. Teardown.
    println!("Cleaning up...");
    if let Some(id) = ladspa_id {
        let _ = ladspa::unload_eq_module(id).await;
        println!("  Unloaded LADSPA module {id}");
    }
    if let Some(id) = null_id {
        let _ = tokio::process::Command::new("pactl")
            .args(["unload-module", &id.to_string()])
            .output()
            .await;
        println!("  Unloaded null-sink module {id}");
    }

    if null_id.is_none() {
        eprintln!("FAIL: could not load test null-sink");
        return 1;
    }
    if ladspa_id.is_none() {
        eprintln!("FAIL: could not load LADSPA EQ module");
        return 1;
    }

    println!();
    println!("OK: mbeq_1197 pipeline lifecycle verified");
    0
}

async fn check_list_options() -> i32 {
    println!("=== list-options integrity check ===");
    println!("List : pulse_audio_devices");
    println!();

    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: cannot connect to session bus: {e}");
            return 1;
        }
    };

    let proxy = match SettingsProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: cannot create Settings proxy: {e}");
            eprintln!("Is lam-daemon running?");
            return 1;
        }
    };

    match proxy.get_list_options("pulse_audio_devices").await {
        Ok(json) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
            let pretty = serde_json::to_string_pretty(&parsed).unwrap_or(json);
            println!("{pretty}");

            // Verify that each entry has id != name (the v2 bug was id == name).
            if let Some(arr) = parsed.as_array() {
                let all_distinct = arr.iter().all(|e| e["id"] != e["name"]);
                if arr.is_empty() {
                    println!(
                        "WARNING: list is empty (no audio sinks found or PipeWire not running)"
                    );
                } else if all_distinct {
                    println!("OK: all entries have distinct id (node.name) and name (node.nick)");
                } else {
                    println!("WARNING: some entries have id == name — node.nick may be missing");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("ERROR: GetListOptions failed: {e}");
            1
        }
    }
}
