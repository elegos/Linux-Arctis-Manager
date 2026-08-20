/// lam-integrity-check — manual verification tool for D-Bus interface behaviour.
///
/// Usage: lam-integrity-check <subcommand>
///
/// Subcommands:
///   settings-signal   Verify that SettingsChanged is emitted after SetSetting()
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

    #[zbus(signal)]
    async fn settings_changed(&self, settings_json: String) -> zbus::Result<()>;
}

#[tokio::main]
async fn main() {
    let subcommand = std::env::args().nth(1).unwrap_or_default();
    let code = match subcommand.as_str() {
        "settings-signal" => check_settings_signal().await,
        _ => {
            eprintln!("Usage: lam-integrity-check <subcommand>");
            eprintln!();
            eprintln!("Subcommands:");
            eprintln!("  settings-signal   Verify SettingsChanged D-Bus signal delivery");
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
