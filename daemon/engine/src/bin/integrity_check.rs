/// lam-integrity-check — manual verification tool for D-Bus interface behaviour.
///
/// Usage: lam-integrity-check <subcommand>
///
/// Subcommands:
///   settings-signal   Verify that SettingsChanged is emitted after SetSetting()
///   list-options      Print GetListOptions("pulse_audio_devices") result
///   general-settings  Print the general section from GetSettings and verify schema
use std::time::Duration;

use futures::StreamExt;
use tokio::time::timeout;
use zbus::proxy;
use zbus::Connection;

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
        _ => {
            eprintln!("Usage: lam-integrity-check <subcommand>");
            eprintln!();
            eprintln!("Subcommands:");
            eprintln!("  settings-signal   Verify SettingsChanged D-Bus signal delivery");
            eprintln!("  list-options      Print GetListOptions(\"pulse_audio_devices\")");
            eprintln!("  general-settings  Verify general section in GetSettings");
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
