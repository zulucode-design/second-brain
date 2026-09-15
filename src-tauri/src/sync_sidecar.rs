//! App-owned lifecycle for the bundled Syncthing process.
//!
//! Callers cross a deliberately small interface: enable/disable and read status. Binary
//! resolution, private home/config, loopback control authentication, startup readiness,
//! crash supervision, and shutdown are implementation details kept here.

use crate::machine_local;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

const SIDECAR_NAME: &str = "syncthing";
const SIDECAR_VERSION: &str = "2.1.5";
const STARTUP_ATTEMPTS: usize = 50;
const STARTUP_WAIT: Duration = Duration::from_millis(100);
const MAX_RESTARTS: u8 = 3;
const STABLE_RUN: Duration = Duration::from_secs(30);
const SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
// A peer can report the previously known index as complete while a fresh scan is still in
// progress. Keep observing a clean cluster long enough for that scan to publish; any late index
// update makes the observation incomplete and resets the latch.
const SYNC_COMPLETION_STABLE_OBSERVATIONS: u8 = 30;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlState {
    version: u8,
    enabled: bool,
    api_key: String,
    gui_port: u16,
    #[serde(default)]
    peer: Option<Peer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Peer {
    device_id: String,
    name: String,
    tailscale_ip: String,
}

fn local_tailscale_ipv4() -> Result<std::net::Ipv4Addr, String> {
    #[cfg(target_os = "windows")]
    let candidates = {
        let mut candidates = vec![PathBuf::from("tailscale.exe")];
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push(
                PathBuf::from(program_files)
                    .join("Tailscale")
                    .join("tailscale.exe"),
            );
        }
        candidates
    };
    #[cfg(not(target_os = "windows"))]
    let candidates = vec![PathBuf::from("tailscale")];
    let output = candidates
        .into_iter()
        .find_map(|executable| {
            std::process::Command::new(executable)
                .args(["ip", "-4"])
                .output()
                .ok()
        })
        .ok_or_else(|| "Tailscale is not installed or could not be found".to_string())?;
    if !output.status.success() {
        return Err("Tailscale is not connected. Connect it before pairing devices".to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| tailscale_ipv4(line.trim()).ok())
        .ok_or_else(|| "Tailscale did not report a usable IPv4 address".to_string())
}

impl ControlState {
    fn new(vault_id: &str) -> Self {
        let prefix = vault_id
            .bytes()
            .filter(|byte| byte.is_ascii_hexdigit())
            .take(4)
            .fold(0_u16, |value, byte| {
                value
                    .wrapping_mul(16)
                    .wrapping_add((byte as char).to_digit(16).unwrap_or(0) as u16)
            });
        Self {
            version: 1,
            enabled: false,
            api_key: uuid::Uuid::new_v4().simple().to_string(),
            gui_port: 18_000 + prefix % 10_000,
            peer: None,
        }
    }
}

#[derive(Debug)]
struct Runtime {
    desired_running: bool,
    generation: u64,
    child: Option<CommandChild>,
    vault_path: Option<PathBuf>,
    device_id: Option<String>,
    last_error: Option<String>,
    restart_count: u8,
    scheduler_generation: u64,
}

pub struct SyncSidecar {
    runtime: Mutex<Runtime>,
    lifecycle: tokio::sync::Mutex<()>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    enabled: bool,
    running: bool,
    version: &'static str,
    device_id: Option<String>,
    error: Option<String>,
    paired: bool,
    peer_name: Option<String>,
    peer_connected: bool,
    vault_id: Option<String>,
}

impl SyncSidecar {
    pub fn new() -> Self {
        Self {
            runtime: Mutex::new(Runtime {
                desired_running: false,
                generation: 0,
                child: None,
                vault_path: None,
                device_id: None,
                last_error: None,
                restart_count: 0,
                scheduler_generation: 0,
            }),
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }

    fn snapshot(&self, enabled: bool) -> Result<SyncStatus, String> {
        let runtime = self.runtime.lock().map_err(|error| error.to_string())?;
        Ok(SyncStatus {
            enabled,
            running: runtime.child.is_some(),
            version: SIDECAR_VERSION,
            device_id: runtime.device_id.clone(),
            error: runtime.last_error.clone(),
            paired: false,
            peer_name: None,
            peer_connected: false,
            vault_id: None,
        })
    }
}

/// Restore only the app-owned process for an explicitly enabled active vault.
pub fn restore_enabled(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Ok(vault) = active_vault(&app) else {
            return;
        };
        let Ok(control) = read_control(&vault) else {
            return;
        };
        if control.enabled {
            if let Err(error) = start(app.clone(), vault, 0).await {
                if let Ok(mut runtime) = app.state::<AppState>().sync_sidecar.runtime.lock() {
                    runtime.last_error = Some(error);
                }
            } else if let Ok(vault) = active_vault(&app) {
                spawn_scheduler(app.clone(), vault);
            }
        }
    });
}

/// Follow a successful vault transition without making vault availability depend on sync.
pub fn activate_vault(app: AppHandle, vault: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let (previous, already_running) = app
            .state::<AppState>()
            .sync_sidecar
            .runtime
            .lock()
            .ok()
            .map(|runtime| (runtime.vault_path.clone(), runtime.child.is_some()))
            .unwrap_or((None, false));
        if previous.as_ref() == Some(&vault) && already_running {
            return;
        }
        if let Some(previous) = previous.filter(|previous| previous != &vault) {
            if let Err(error) = stop(&app, &previous).await {
                log::warn!("Could not stop sync for the previous vault: {error}");
            }
        }
        let Ok(control) = read_control(&vault) else {
            return;
        };
        if control.enabled {
            match start(app.clone(), vault.clone(), 0).await {
                Ok(()) => spawn_scheduler(app.clone(), vault),
                Err(error) => {
                    if let Ok(mut runtime) = app.state::<AppState>().sync_sidecar.runtime.lock() {
                        runtime.last_error = Some(error);
                    }
                }
            }
        }
    });
}

fn spawn_scheduler(app: AppHandle, vault: PathBuf) {
    let token = {
        let state = app.state::<AppState>();
        let Ok(mut runtime) = state.sync_sidecar.runtime.lock() else {
            return;
        };
        runtime.scheduler_generation = runtime.scheduler_generation.wrapping_add(1);
        runtime.scheduler_generation
    };
    tauri::async_runtime::spawn(async move {
        loop {
            // Align both machines to UTC boundaries instead of measuring from app launch.
            // Independent launch timers may never overlap, leaving both safe paused folders
            // unable to exchange data.
            tokio::time::sleep(until_next_sync_window()).await;
            let current = app
                .state::<AppState>()
                .sync_sidecar
                .runtime
                .lock()
                .map(|runtime| runtime.scheduler_generation == token && runtime.desired_running)
                .unwrap_or(false);
            if !current {
                return;
            }
            let Ok(control) = read_control(&vault) else {
                continue;
            };
            let Some(peer) = control.peer.clone() else {
                continue;
            };
            let running = app
                .state::<AppState>()
                .sync_sidecar
                .snapshot(true)
                .map(|status| status.running)
                .unwrap_or(false);
            if !running && start(app.clone(), vault.clone(), 0).await.is_err() {
                continue;
            }
            let run_app = app.clone();
            let run_vault = vault.clone();
            std::thread::spawn(move || run_sync(run_app, run_vault, control, peer));
        }
    });
}

fn until_next_sync_window() -> Duration {
    let interval = SYNC_INTERVAL.as_millis();
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Duration::from_millis((interval - elapsed % interval) as u64)
}

fn active_vault(app: &AppHandle) -> Result<PathBuf, String> {
    app.state::<AppState>()
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .active_vault
        .clone()
        .map(PathBuf::from)
        .ok_or_else(|| "No active vault".to_string())
}

fn control_path(vault: &Path) -> Result<PathBuf, String> {
    Ok(machine_local::vault_dir(vault)?.join("sync-control.json"))
}

fn read_control(vault: &Path) -> Result<ControlState, String> {
    let path = control_path(vault)?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let state: ControlState = serde_json::from_str(&contents)
                .map_err(|error| format!("Sync control state is invalid: {error}"))?;
            if state.version != 1 || state.api_key.is_empty() || state.gui_port == 0 {
                return Err("Sync control state is incompatible".to_string());
            }
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let vault_id = machine_local::vault_id(vault)?;
            let state = ControlState::new(&vault_id);
            write_control(vault, &state)?;
            Ok(state)
        }
        Err(error) => Err(format!("Could not read sync control state: {error}")),
    }
}

fn write_control(vault: &Path, state: &ControlState) -> Result<(), String> {
    let path = control_path(vault)?;
    let data = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
    crate::durable::replace(&path, &data, crate::durable::Mode::Private)
}

fn endpoint(control: &ControlState, path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", control.gui_port)
}

async fn system_status(control: &ControlState) -> Result<String, String> {
    let response = reqwest::Client::new()
        .get(endpoint(control, "/rest/system/status"))
        .header("X-API-Key", &control.api_key)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Syncthing status failed: HTTP {}",
            response.status()
        ));
    }
    let value: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    value
        .get("myID")
        .and_then(|id| id.as_str())
        .map(str::to_string)
        .ok_or_else(|| "Syncthing did not report its device ID".to_string())
}

async fn peer_is_connected(control: &ControlState, peer_id: &str) -> bool {
    let Ok(response) = reqwest::Client::new()
        .get(endpoint(control, "/rest/system/connections"))
        .header("X-API-Key", &control.api_key)
        .send()
        .await
    else {
        return false;
    };
    let Ok(value) = response.json::<serde_json::Value>().await else {
        return false;
    };
    value
        .pointer(&format!("/connections/{peer_id}/connected"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn tailscale_ipv4(value: &str) -> Result<std::net::Ipv4Addr, String> {
    let address: std::net::Ipv4Addr = value
        .parse()
        .map_err(|_| "Enter the peer's Tailscale IPv4 address".to_string())?;
    let octets = address.octets();
    if octets[0] != 100 || !(64..=127).contains(&octets[1]) {
        return Err("The peer address must be in Tailscale's 100.64.0.0/10 range".to_string());
    }
    Ok(address)
}

fn valid_device_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 8
        && groups.iter().all(|group| {
            group.len() == 7
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))
        })
}

async fn apply_pairing(control: &ControlState, vault: &Path, peer: &Peer) -> Result<bool, String> {
    let local_address = local_tailscale_ipv4()?;
    let client = reqwest::Client::new();
    let mut config: serde_json::Value = client
        .get(endpoint(control, "/rest/config"))
        .header("X-API-Key", &control.api_key)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| "Syncthing returned an invalid configuration".to_string())?;
    root.insert(
        "devices".into(),
        serde_json::json!([{
            "deviceID": peer.device_id, "name": peer.name,
            "addresses": [format!("tcp://{}:22000", peer.tailscale_ip)],
            "autoAcceptFolders": false
        }]),
    );
    root.insert(
        "folders".into(),
        serde_json::json!([{
            "id": machine_local::vault_id(vault)?, "label": "Second Brain Vault",
            "path": vault.to_string_lossy(), "type": "sendreceive", "paused": true,
            "devices": [{"deviceID": peer.device_id}], "fsWatcherEnabled": true
        }]),
    );
    if let Some(options) = root
        .get_mut("options")
        .and_then(|value| value.as_object_mut())
    {
        for key in [
            "globalAnnounceEnabled",
            "localAnnounceEnabled",
            "relaysEnabled",
            "natEnabled",
            "crashReportingEnabled",
        ] {
            options.insert(key.into(), false.into());
        }
        options.insert("urAccepted".into(), (-1).into());
        options.insert(
            "listenAddresses".into(),
            serde_json::json!([format!("tcp://{local_address}:22000")]),
        );
    }
    client
        .put(endpoint(control, "/rest/config"))
        .header("X-API-Key", &control.api_key)
        .json(&config)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| format!("Syncthing rejected the pairing: {error}"))?;
    let restart: serde_json::Value = client
        .get(endpoint(control, "/rest/config/restart-required"))
        .header("X-API-Key", &control.api_key)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let requires_restart = restart
        .get("requiresRestart")
        .and_then(|value| value.as_bool())
        == Some(true);
    Ok(requires_restart)
}

async fn generate_if_needed(app: &AppHandle, vault: &Path) -> Result<PathBuf, String> {
    let home = machine_local::syncthing_dir(vault)?;
    if home.join("config.xml").exists() {
        return Ok(home);
    }
    let args = vec![
        "generate".to_string(),
        format!("--home={}", home.to_string_lossy()),
        "--no-port-probing".to_string(),
    ];
    let output = app
        .shell()
        .sidecar(SIDECAR_NAME)
        .map_err(|error| error.to_string())?
        .args(args)
        .output()
        .await
        .map_err(|error| format!("Could not initialize bundled Syncthing: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Bundled Syncthing initialization failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(home)
}

fn harden_generated_config_xml(
    contents: &str,
    control: &ControlState,
    local_address: std::net::Ipv4Addr,
) -> Result<Vec<u8>, String> {
    use quick_xml::events::{BytesText, Event};

    fn replacement(
        path: &[Vec<u8>],
        control: &ControlState,
        local_address: std::net::Ipv4Addr,
    ) -> Option<(&'static str, String)> {
        if path.len() != 3 || path[0] != b"configuration" {
            return None;
        }
        match (path[1].as_slice(), path[2].as_slice()) {
            (b"gui", b"address") => {
                Some(("gui/address", format!("127.0.0.1:{}", control.gui_port)))
            }
            (b"gui", b"apikey") => Some(("gui/apikey", control.api_key.clone())),
            (b"options", b"listenAddress") => Some((
                "options/listenAddress",
                format!("tcp://{local_address}:22000"),
            )),
            (b"options", b"globalAnnounceEnabled") => {
                Some(("options/globalAnnounceEnabled", "false".into()))
            }
            (b"options", b"localAnnounceEnabled") => {
                Some(("options/localAnnounceEnabled", "false".into()))
            }
            (b"options", b"relaysEnabled") => Some(("options/relaysEnabled", "false".into())),
            (b"options", b"startBrowser") => Some(("options/startBrowser", "false".into())),
            (b"options", b"natEnabled") => Some(("options/natEnabled", "false".into())),
            (b"options", b"urAccepted") => Some(("options/urAccepted", "-1".into())),
            (b"options", b"autoUpgradeIntervalH") => {
                Some(("options/autoUpgradeIntervalH", "0".into()))
            }
            (b"options", b"crashReportingEnabled") => {
                Some(("options/crashReportingEnabled", "false".into()))
            }
            (b"options", b"announceLANAddresses") => {
                Some(("options/announceLANAddresses", "false".into()))
            }
            _ => None,
        }
    }

    let mut reader = quick_xml::Reader::from_str(contents);
    reader.config_mut().trim_text(false);
    let mut writer = quick_xml::Writer::new(Vec::with_capacity(contents.len()));
    let mut path = Vec::<Vec<u8>>::new();
    let mut replaced = std::collections::BTreeSet::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("Generated Syncthing configuration is invalid: {error}"))?;
        match event {
            Event::Start(start) => {
                path.push(start.name().as_ref().to_vec());
                writer
                    .write_event(Event::Start(start))
                    .map_err(|error| error.to_string())?;
            }
            Event::Text(text) => {
                if let Some((field, value)) = replacement(&path, control, local_address) {
                    replaced.insert(field);
                    writer
                        .write_event(Event::Text(BytesText::new(&value)))
                        .map_err(|error| error.to_string())?;
                } else {
                    writer
                        .write_event(Event::Text(text))
                        .map_err(|error| error.to_string())?;
                }
            }
            Event::End(end) => {
                writer
                    .write_event(Event::End(end))
                    .map_err(|error| error.to_string())?;
                path.pop();
            }
            Event::Eof => break,
            other => writer
                .write_event(other)
                .map_err(|error| error.to_string())?,
        }
    }
    const REQUIRED_FIELDS: usize = 12;
    if replaced.len() != REQUIRED_FIELDS {
        return Err(format!(
            "Generated Syncthing configuration is missing required safety fields (found {} of {REQUIRED_FIELDS})",
            replaced.len()
        ));
    }
    Ok(writer.into_inner())
}

fn harden_generated_config(
    home: &Path,
    control: &ControlState,
    local_address: std::net::Ipv4Addr,
) -> Result<(), String> {
    let path = home.join("config.xml");
    let contents = std::fs::read_to_string(&path)
        .map_err(|error| format!("Could not read generated Syncthing configuration: {error}"))?;
    let hardened = harden_generated_config_xml(&contents, control, local_address)?;
    crate::durable::replace(&path, &hardened, crate::durable::Mode::Private)
        .map_err(|error| format!("Could not harden Syncthing configuration: {error}"))
}

fn start(
    app: AppHandle,
    vault: PathBuf,
    restart_count: u8,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
    Box::pin(async move {
        let lifecycle_state = app.state::<AppState>();
        let _lifecycle = lifecycle_state.sync_sidecar.lifecycle.lock().await;
        {
            let runtime = lifecycle_state
                .sync_sidecar
                .runtime
                .lock()
                .map_err(|error| error.to_string())?;
            if runtime.child.is_some() {
                return if runtime.vault_path.as_ref() == Some(&vault) {
                    Ok(())
                } else {
                    Err("Syncthing is still attached to another vault".to_string())
                };
            }
        }
        let control = read_control(&vault)?;
        let home = generate_if_needed(&app, &vault).await?;
        let local_address = local_tailscale_ipv4()?;
        harden_generated_config(&home, &control, local_address)?;
        let args = vec![
            "serve".to_string(),
            format!("--home={}", home.to_string_lossy()),
            "--no-browser".to_string(),
            "--paused".to_string(),
            "--no-restart".to_string(),
            "--no-upgrade".to_string(),
            "--no-port-probing".to_string(),
            format!(
                "--log-file={}",
                home.join("syncthing.log").to_string_lossy()
            ),
            "--log-max-size=5242880".to_string(),
            "--log-max-old-files=2".to_string(),
        ];
        let (mut events, child) = app
            .shell()
            .sidecar(SIDECAR_NAME)
            .map_err(|error| error.to_string())?
            .args(args)
            .env("STGUIADDRESS", format!("127.0.0.1:{}", control.gui_port))
            .env("STGUIAPIKEY", &control.api_key)
            .env("STVERSIONEXTRA", "Second Brain managed sidecar")
            // Run as one process. Otherwise Syncthing's monitor spawns a worker that survives
            // kill() on Windows, keeps the lock and escapes the watchdog (#104). The app
            // supervises restarts itself. ponytail: hidden switch verified on v2.1.5 only;
            // recheck with `tasklist`/`ps` when bumping the pinned Syncthing.
            .env("STMONITORED", "yes")
            .spawn()
            .map_err(|error| format!("Could not start bundled Syncthing: {error}"))?;
        if let Err(error) =
            crate::sync_watchdog::spawn(child.pid(), control.gui_port, &control.api_key)
        {
            let _ = child.kill();
            return Err(error);
        }

        let generation = {
            let state = app.state::<AppState>();
            let mut runtime = state
                .sync_sidecar
                .runtime
                .lock()
                .map_err(|error| error.to_string())?;
            runtime.generation = runtime.generation.wrapping_add(1);
            runtime.desired_running = true;
            runtime.child = Some(child);
            runtime.vault_path = Some(vault.clone());
            runtime.last_error = None;
            runtime.restart_count = restart_count;
            runtime.generation
        };

        let monitor_app = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                match event {
                    CommandEvent::Stderr(bytes) => {
                        log::warn!("Syncthing: {}", String::from_utf8_lossy(&bytes).trim())
                    }
                    CommandEvent::Error(error) => log::error!("Syncthing process error: {error}"),
                    CommandEvent::Terminated(status) => {
                        let should_restart = {
                            let state = monitor_app.state::<AppState>();
                            let Ok(mut runtime) = state.sync_sidecar.runtime.lock() else {
                                return;
                            };
                            if runtime.generation != generation {
                                return;
                            }
                            runtime.child = None;
                            if runtime.desired_running {
                                runtime.last_error = Some(format!(
                                    "Syncthing stopped unexpectedly ({:?})",
                                    status.code
                                ));
                            }
                            if runtime.desired_running && runtime.restart_count >= MAX_RESTARTS {
                                runtime.desired_running = false;
                                runtime.last_error = Some(format!(
                                    "Syncthing stopped after {MAX_RESTARTS} restart attempts"
                                ));
                                false
                            } else {
                                runtime.desired_running
                            }
                        };
                        if should_restart {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            if let Err(error) =
                                start(monitor_app.clone(), vault.clone(), restart_count + 1).await
                            {
                                if let Ok(mut runtime) =
                                    monitor_app.state::<AppState>().sync_sidecar.runtime.lock()
                                {
                                    runtime.last_error = Some(error);
                                }
                            }
                        }
                        return;
                    }
                    _ => {}
                }
            }
        });

        let mut last = None;
        for _ in 0..STARTUP_ATTEMPTS {
            match system_status(&control).await {
                Ok(device_id) => {
                    {
                        let state = app.state::<AppState>();
                        let mut runtime = state
                            .sync_sidecar
                            .runtime
                            .lock()
                            .map_err(|error| error.to_string())?;
                        if runtime.generation == generation {
                            runtime.device_id = Some(device_id);
                        }
                    }
                    // A process that merely reached its REST API is not yet a stable run.
                    // Reset the bounded crash budget only after it stays alive long enough.
                    let stable_app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(STABLE_RUN).await;
                        if let Ok(mut runtime) =
                            stable_app.state::<AppState>().sync_sidecar.runtime.lock()
                        {
                            if runtime.generation == generation && runtime.child.is_some() {
                                runtime.restart_count = 0;
                            }
                        }
                    });
                    return Ok(());
                }
                Err(error) => last = Some(error),
            }
            tokio::time::sleep(STARTUP_WAIT).await;
        }
        let child = {
            let state = app.state::<AppState>();
            let mut runtime = state
                .sync_sidecar
                .runtime
                .lock()
                .map_err(|error| error.to_string())?;
            if runtime.generation == generation {
                runtime.desired_running = false;
                runtime.generation = runtime.generation.wrapping_add(1);
                runtime.child.take()
            } else {
                None
            }
        };
        if let Some(child) = child {
            let _ = child.kill();
        }
        Err(format!(
            "Bundled Syncthing did not become ready: {}",
            last.unwrap_or_default()
        ))
    })
}

async fn stop(app: &AppHandle, vault: &Path) -> Result<(), String> {
    let lifecycle_state = app.state::<AppState>();
    let _lifecycle = lifecycle_state.sync_sidecar.lifecycle.lock().await;
    let control = read_control(vault)?;
    {
        let state = app.state::<AppState>();
        let mut runtime = state
            .sync_sidecar
            .runtime
            .lock()
            .map_err(|error| error.to_string())?;
        runtime.desired_running = false;
        runtime.generation = runtime.generation.wrapping_add(1);
        runtime.scheduler_generation = runtime.scheduler_generation.wrapping_add(1);
    }
    let _ = reqwest::Client::new()
        .post(endpoint(&control, "/rest/system/shutdown"))
        .header("X-API-Key", &control.api_key)
        .send()
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let child = app
        .state::<AppState>()
        .sync_sidecar
        .runtime
        .lock()
        .map_err(|error| error.to_string())?
        .child
        .take();
    if let Some(child) = child {
        let _ = child.kill();
    }
    Ok(())
}

/// Build the status payload at the command boundary, where machine-local and peer state belong.
async fn enriched_status(
    app: &AppHandle,
    vault: &Path,
    control: &ControlState,
    enabled: bool,
) -> Result<SyncStatus, String> {
    let mut status = app.state::<AppState>().sync_sidecar.snapshot(enabled)?;
    status.vault_id = Some(machine_local::vault_id(vault)?);
    status.paired = control.peer.is_some();
    status.peer_name = control.peer.as_ref().map(|peer| peer.name.clone());
    if status.running {
        if let Some(peer) = &control.peer {
            status.peer_connected = peer_is_connected(control, &peer.device_id).await;
        }
    }
    Ok(status)
}

#[tauri::command]
pub async fn sync_status(app: AppHandle) -> Result<SyncStatus, String> {
    let vault = active_vault(&app)?;
    let control = read_control(&vault)?;
    enriched_status(&app, &vault, &control, control.enabled).await
}

#[tauri::command]
pub async fn sync_set_enabled(app: AppHandle, enabled: bool) -> Result<SyncStatus, String> {
    let vault = active_vault(&app)?;
    let mut control = read_control(&vault)?;
    if enabled {
        let already_running = app
            .state::<AppState>()
            .sync_sidecar
            .runtime
            .lock()
            .map_err(|error| error.to_string())?
            .child
            .is_some();
        if !already_running {
            start(app.clone(), vault.clone(), 0).await?;
        }
        spawn_scheduler(app.clone(), vault.clone());
    } else {
        stop(&app, &vault).await?;
    }
    control.enabled = enabled;
    write_control(&vault, &control)?;
    enriched_status(&app, &vault, &control, enabled).await
}

#[tauri::command]
pub async fn sync_pair(
    app: AppHandle,
    device_id: String,
    name: String,
    tailscale_ip: String,
    vault_id: String,
) -> Result<SyncStatus, String> {
    let vault = active_vault(&app)?;
    let local_vault_id = machine_local::vault_id(&vault)?;
    if vault_id.trim() != local_vault_id {
        return Err("The other machine has a different vault identity. Copy the existing vault to this machine before pairing".to_string());
    }
    let device_id = device_id.trim().to_ascii_uppercase();
    if !valid_device_id(&device_id) {
        return Err(
            "The Syncthing device ID must contain eight groups of seven characters".to_string(),
        );
    }
    let tailscale_ip = tailscale_ipv4(tailscale_ip.trim())?.to_string();
    let name = name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err("Give the paired device a name of 1–80 characters".to_string());
    }
    let mut control = read_control(&vault)?;
    if !app
        .state::<AppState>()
        .sync_sidecar
        .snapshot(control.enabled)?
        .running
    {
        start(app.clone(), vault.clone(), 0).await?;
    }
    let peer = Peer {
        device_id,
        name: name.to_string(),
        tailscale_ip,
    };
    let requires_restart = apply_pairing(&control, &vault, &peer).await?;
    if requires_restart {
        stop(&app, &vault).await?;
        start(app.clone(), vault.clone(), 0).await?;
    }
    control.peer = Some(peer);
    control.enabled = true;
    write_control(&vault, &control)?;
    spawn_scheduler(app.clone(), vault);
    sync_status(app).await
}

fn set_folder_paused(
    client: &reqwest::blocking::Client,
    control: &ControlState,
    folder_id: &str,
    paused: bool,
) -> Result<(), String> {
    client
        .patch(endpoint(
            control,
            &format!("/rest/config/folders/{folder_id}"),
        ))
        .header("X-API-Key", &control.api_key)
        .json(&serde_json::json!({ "paused": paused }))
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| {
            format!(
                "Could not {} the sync folder: {error}",
                if paused { "pause" } else { "resume" }
            )
        })?;
    Ok(())
}

fn set_device_paused(
    client: &reqwest::blocking::Client,
    control: &ControlState,
    device_id: &str,
    paused: bool,
) -> Result<(), String> {
    client
        .patch(endpoint(
            control,
            &format!("/rest/config/devices/{device_id}"),
        ))
        .header("X-API-Key", &control.api_key)
        .json(&serde_json::json!({ "paused": paused }))
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| {
            format!(
                "Could not {} the paired device: {error}",
                if paused { "pause" } else { "resume" }
            )
        })?;
    Ok(())
}

/// Crash-orphaned `durable::replace` temporaries must never replicate (#99).
const DURABLE_TEMP_IGNORE: &str = "(?d).*.tmp";

/// The `.stignore` lines to write, or `None` if the pattern is already there.
fn with_durable_temp_ignore(mut lines: Vec<String>) -> Option<Vec<String>> {
    if lines.iter().any(|line| line.trim() == DURABLE_TEMP_IGNORE) {
        return None;
    }
    lines.push(DURABLE_TEMP_IGNORE.to_string());
    Some(lines)
}

fn ensure_durable_temp_ignored(
    client: &reqwest::blocking::Client,
    control: &ControlState,
    folder_id: &str,
) -> Result<(), String> {
    let url = endpoint(control, &format!("/rest/db/ignores?folder={folder_id}"));
    let current: serde_json::Value = client
        .get(&url)
        .header("X-API-Key", &control.api_key)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json())
        .map_err(|error| format!("Could not read the sync ignore patterns: {error}"))?;
    let lines = current
        .get("ignore")
        .and_then(|value| value.as_array())
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let Some(lines) = with_durable_temp_ignore(lines) else {
        return Ok(());
    };
    client
        .post(&url)
        .header("X-API-Key", &control.api_key)
        .json(&serde_json::json!({ "ignore": lines }))
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Could not update the sync ignore patterns: {error}"))?;
    Ok(())
}

struct FolderPauseGuard<'a> {
    client: &'a reqwest::blocking::Client,
    control: &'a ControlState,
    folder_id: &'a str,
    device_id: &'a str,
    armed: bool,
}

impl FolderPauseGuard<'_> {
    fn pause(mut self) -> Result<(), String> {
        let folder = set_folder_paused(self.client, self.control, self.folder_id, true);
        let device = set_device_paused(self.client, self.control, self.device_id, true);
        let result = match (folder, device) {
            (Ok(()), Ok(())) => Ok(()),
            (folder, device) => Err([folder.err(), device.err()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ")),
        };
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

impl Drop for FolderPauseGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = set_folder_paused(self.client, self.control, self.folder_id, true) {
            log::error!("Sync safety cleanup failed: {error}");
        }
        if let Err(error) = set_device_paused(self.client, self.control, self.device_id, true) {
            log::error!("Sync device safety cleanup failed: {error}");
        }
    }
}

fn peer_connected(
    client: &reqwest::blocking::Client,
    control: &ControlState,
    peer_id: &str,
) -> Result<bool, String> {
    let value: serde_json::Value = client
        .get(endpoint(control, "/rest/system/connections"))
        .header("X-API-Key", &control.api_key)
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .map_err(|error| error.to_string())?;
    Ok(value
        .pointer(&format!("/connections/{peer_id}/connected"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false))
}

fn convergence_observation_is_complete(
    local_status: &serde_json::Value,
    remote_completion: Option<&serde_json::Value>,
    peer_id: &str,
) -> bool {
    let idle = local_status.get("state").and_then(|value| value.as_str()) == Some("idle");
    let needed = local_status
        .get("needTotalItems")
        .and_then(|value| value.as_u64())
        .unwrap_or(1);
    let received_peer_index = local_status
        .get("remoteSequence")
        .and_then(|value| value.as_object())
        .is_some_and(|sequences| sequences.contains_key(peer_id));
    let peer_complete = remote_completion.is_some_and(|completion| {
        completion
            .get("remoteState")
            .and_then(|value| value.as_str())
            == Some("valid")
            && completion.get("needItems").and_then(|value| value.as_u64()) == Some(0)
            && completion
                .get("needDeletes")
                .and_then(|value| value.as_u64())
                == Some(0)
            && completion.get("needBytes").and_then(|value| value.as_u64()) == Some(0)
    });
    idle && needed == 0 && pull_errors(local_status) == 0 && received_peer_index && peer_complete
}

fn pull_errors(local_status: &serde_json::Value) -> u64 {
    local_status
        .get("pullErrors")
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
}

// Syncthing retries failed pulls about once a minute and can briefly report none between
// attempts, so errors seen within this many observations (60 s at 500 ms) still explain a failure.
const PULL_ERROR_EXPLAINS_FAILURE: usize = 120;

/// One guarded run's view of convergence. Pull errors are not terminal (#101): after an
/// interruption Syncthing reports them while the peer reconnects and re-sends its index, then
/// retries them itself. They block completion and only name the failure if the deadline arrives
/// while they are still recent.
#[derive(Default)]
struct SyncProgress {
    completion: CompletionLatch,
    observations: usize,
    last_pull_error: Option<usize>,
}

impl SyncProgress {
    fn observe(
        &mut self,
        local_status: &serde_json::Value,
        remote_completion: Option<&serde_json::Value>,
        peer_id: &str,
    ) -> bool {
        self.observations += 1;
        if pull_errors(local_status) > 0 {
            self.last_pull_error = Some(self.observations);
        }
        self.completion.observe(convergence_observation_is_complete(
            local_status,
            remote_completion,
            peer_id,
        ))
    }

    fn failure(&self) -> &'static str {
        match self.last_pull_error {
            Some(at) if self.observations - at < PULL_ERROR_EXPLAINS_FAILURE => {
                "Syncthing reported one or more file errors"
            }
            _ => "Sync did not converge within five minutes",
        }
    }
}

#[derive(Default)]
struct CompletionLatch {
    consecutive_complete: u8,
    confirmed: bool,
}

impl CompletionLatch {
    fn observe(&mut self, complete: bool) -> bool {
        if self.confirmed {
            return true;
        }
        self.consecutive_complete = if complete {
            self.consecutive_complete.saturating_add(1)
        } else {
            0
        };
        self.confirmed = self.consecutive_complete >= SYNC_COMPLETION_STABLE_OBSERVATIONS;
        self.confirmed
    }
}

fn wait_for_sync(
    client: &reqwest::blocking::Client,
    control: &ControlState,
    folder_id: &str,
    peer_id: &str,
) -> Result<(), String> {
    for _ in 0..60 {
        if peer_connected(client, control, peer_id)? {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !peer_connected(client, control, peer_id)? {
        return Err("The paired device is not reachable over Tailscale".to_string());
    }
    client
        .post(endpoint(control, "/rest/db/scan"))
        .header("X-API-Key", &control.api_key)
        .query(&[("folder", folder_id)])
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| format!("Syncthing could not scan the vault: {error}"))?;
    let mut progress = SyncProgress::default();
    for _ in 0..600 {
        let value: serde_json::Value = client
            .get(endpoint(control, "/rest/db/status"))
            .header("X-API-Key", &control.api_key)
            .query(&[("folder", folder_id)])
            .send()
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .json()
            .map_err(|error| error.to_string())?;
        let completion_response = client
            .get(endpoint(control, "/rest/db/completion"))
            .header("X-API-Key", &control.api_key)
            .query(&[("folder", folder_id), ("device", peer_id)])
            .send()
            .map_err(|error| error.to_string())?;
        let remote_completion = if completion_response.status() == reqwest::StatusCode::NOT_FOUND {
            None
        } else {
            Some(
                completion_response
                    .error_for_status()
                    .map_err(|error| error.to_string())?
                    .json::<serde_json::Value>()
                    .map_err(|error| error.to_string())?,
            )
        };
        if progress.observe(&value, remote_completion.as_ref(), peer_id) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(progress.failure().to_string())
}

fn run_sync(app: AppHandle, vault: PathBuf, control: ControlState, peer: Peer) {
    use tauri::Emitter;
    let state = app.state::<AppState>();
    let terminal = match state
        .bulk_mutation
        .acquire(&state.note_mutation, &state.vault_activity)
    {
        Err(error) => crate::bulk_mutation::BulkMutationTerminal::failure(error),
        Ok(_lease) => {
            let (backup_dir, max_count) = match state.config.lock() {
                Ok(config) => match crate::backup::get_backup_dir(&config.backup_location) {
                    Ok(dir) => (dir, config.backup_max_count),
                    Err(error) => {
                        let _ = app.emit(
                            "sync-done",
                            crate::bulk_mutation::BulkMutationTerminal::failure(error),
                        );
                        return;
                    }
                },
                Err(error) => {
                    let _ = app.emit(
                        "sync-done",
                        crate::bulk_mutation::BulkMutationTerminal::failure(error.to_string()),
                    );
                    return;
                }
            };
            let backup_inside_vault = std::fs::canonicalize(&backup_dir)
                .ok()
                .zip(std::fs::canonicalize(&vault).ok())
                .is_some_and(|(backup, vault)| backup.starts_with(vault));
            if backup_inside_vault {
                let _ = app.emit(
                    "sync-done",
                    crate::bulk_mutation::BulkMutationTerminal::failure(
                        "Sync aborted: choose a backup folder outside the vault",
                    ),
                );
                return;
            }
            match crate::backup::create_pre_sync_backup(&vault.to_string_lossy(), &backup_dir) {
                Err(error) => crate::bulk_mutation::BulkMutationTerminal::failure(format!(
                    "Sync aborted because its safety backup failed: {error}"
                )),
                Ok(_) => {
                    let _ = crate::backup::cleanup_old_backups(&backup_dir, max_count);
                    let client = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .build()
                        .unwrap();
                    let folder_id = match machine_local::vault_id(&vault) {
                        Ok(id) => id,
                        Err(error) => {
                            let _ = app.emit(
                                "sync-done",
                                crate::bulk_mutation::BulkMutationTerminal::failure(error),
                            );
                            return;
                        }
                    };
                    match ensure_durable_temp_ignored(&client, &control, &folder_id)
                        .and_then(|()| set_folder_paused(&client, &control, &folder_id, false))
                    {
                        Err(error) => crate::bulk_mutation::BulkMutationTerminal::failure(error),
                        Ok(()) => {
                            if let Err(error) =
                                set_device_paused(&client, &control, &peer.device_id, false)
                            {
                                let _ = set_folder_paused(&client, &control, &folder_id, true);
                                let _ = app.emit(
                                    "sync-done",
                                    crate::bulk_mutation::BulkMutationTerminal::failure(error),
                                );
                                return;
                            }
                            let pause = FolderPauseGuard {
                                client: &client,
                                control: &control,
                                folder_id: &folder_id,
                                device_id: &peer.device_id,
                                armed: true,
                            };
                            let result =
                                wait_for_sync(&client, &control, &folder_id, &peer.device_id);
                            let paused = pause.pause();
                            let reconciliation = crate::commands::reconcile_bulk_projections(
                                &state,
                                &vault.to_string_lossy(),
                            );
                            match (result, paused, reconciliation) {
                                (Ok(()), Ok(()), Ok(())) => {
                                    crate::bulk_mutation::BulkMutationTerminal::success()
                                }
                                (sync, pause, projection) => {
                                    crate::bulk_mutation::BulkMutationTerminal::changed_incomplete(
                                        [sync.err(), pause.err(), projection.err()]
                                            .into_iter()
                                            .flatten()
                                            .collect::<Vec<_>>()
                                            .join("; "),
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    let _ = app.emit("sync-done", terminal);
}

#[tauri::command]
pub async fn sync_now(app: AppHandle) -> Result<(), String> {
    let vault = active_vault(&app)?;
    let control = read_control(&vault)?;
    let peer = control
        .peer
        .clone()
        .ok_or_else(|| "Pair another device before syncing".to_string())?;
    if !app
        .state::<AppState>()
        .sync_sidecar
        .snapshot(control.enabled)?
        .running
    {
        start(app.clone(), vault.clone(), 0).await?;
    }
    std::thread::spawn(move || run_sync(app, vault, control, peer));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        convergence_observation_is_complete, harden_generated_config_xml, read_control,
        tailscale_ipv4, until_next_sync_window, valid_device_id, with_durable_temp_ignore,
        write_control, CompletionLatch, ControlState, SyncProgress, DURABLE_TEMP_IGNORE,
        PULL_ERROR_EXPLAINS_FAILURE, SYNC_COMPLETION_STABLE_OBSERVATIONS, SYNC_INTERVAL,
    };

    #[test]
    fn durable_temp_ignore_is_appended_once_and_keeps_user_patterns() {
        let user = vec!["// mine".to_string(), "private/".to_string()];
        let updated = with_durable_temp_ignore(user.clone()).unwrap();
        assert_eq!(&updated[..2], &user[..]);
        assert_eq!(updated[2], DURABLE_TEMP_IGNORE);
        assert_eq!(with_durable_temp_ignore(updated), None);
        assert_eq!(
            with_durable_temp_ignore(Vec::new()),
            Some(vec![DURABLE_TEMP_IGNORE.to_string()])
        );
    }

    fn vault() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("sync-sidecar-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn control_state_is_machine_local_and_stable() {
        let vault = vault();
        let first = read_control(&vault).unwrap();
        let second = read_control(&vault).unwrap();
        assert_eq!(first.api_key, second.api_key);
        assert_eq!(first.gui_port, second.gui_port);
        assert!(!vault.join(".helixnotes/sync-control.json").exists());
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn incompatible_control_state_fails_closed() {
        let vault = vault();
        let mut state = read_control(&vault).unwrap();
        state.version = 99;
        write_control(&vault, &state).unwrap();
        assert!(read_control(&vault).unwrap_err().contains("incompatible"));
        std::fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn pairing_requires_an_explicit_identity_on_the_tailnet() {
        assert!(valid_device_id(
            "ABCDEFG-ABCDEFG-ABCDEFG-ABCDEFG-ABCDEFG-ABCDEFG-ABCDEFG-ABCDEFG"
        ));
        assert!(!valid_device_id("anything-on-the-tailnet"));
        assert!(tailscale_ipv4("100.64.0.1").is_ok());
        assert!(tailscale_ipv4("100.127.255.254").is_ok());
        assert!(tailscale_ipv4("192.168.1.10").is_err());
        assert!(tailscale_ipv4("100.128.0.1").is_err());
    }

    #[test]
    fn scheduled_windows_are_aligned_and_never_more_than_one_interval_away() {
        assert!(until_next_sync_window() <= SYNC_INTERVAL);
    }

    #[test]
    fn generated_config_is_hardened_before_the_first_serve() {
        let generated = r#"<configuration version="52">
    <device id="LOCAL"><address>dynamic</address><paused>false</paused></device>
    <gui enabled="true"><address>127.0.0.1:8384</address><apikey>generated-key</apikey></gui>
    <options>
        <listenAddress>default</listenAddress>
        <globalAnnounceEnabled>true</globalAnnounceEnabled>
        <localAnnounceEnabled>true</localAnnounceEnabled>
        <relaysEnabled>true</relaysEnabled>
        <startBrowser>true</startBrowser>
        <natEnabled>true</natEnabled>
        <urAccepted>0</urAccepted>
        <autoUpgradeIntervalH>12</autoUpgradeIntervalH>
        <crashReportingEnabled>true</crashReportingEnabled>
        <announceLANAddresses>true</announceLANAddresses>
    </options>
</configuration>"#;
        let control = ControlState {
            version: 1,
            enabled: true,
            api_key: "app-owned-key".into(),
            gui_port: 22046,
            peer: None,
        };

        let hardened = String::from_utf8(
            harden_generated_config_xml(generated, &control, "100.124.210.13".parse().unwrap())
                .unwrap(),
        )
        .unwrap();

        assert!(hardened.contains("<address>127.0.0.1:22046</address>"));
        assert!(hardened.contains("<apikey>app-owned-key</apikey>"));
        assert!(hardened.contains("<listenAddress>tcp://100.124.210.13:22000</listenAddress>"));
        for element in [
            "globalAnnounceEnabled",
            "localAnnounceEnabled",
            "relaysEnabled",
            "startBrowser",
            "natEnabled",
            "crashReportingEnabled",
            "announceLANAddresses",
        ] {
            assert!(hardened.contains(&format!("<{element}>false</{element}>")));
        }
        assert!(hardened.contains("<urAccepted>-1</urAccepted>"));
        assert!(hardened.contains("<autoUpgradeIntervalH>0</autoUpgradeIntervalH>"));
        assert!(hardened.contains("<device id=\"LOCAL\"><address>dynamic</address>"));
    }

    #[test]
    fn generated_config_with_missing_safety_fields_fails_closed() {
        let control = ControlState {
            version: 1,
            enabled: true,
            api_key: "app-owned-key".into(),
            gui_port: 22046,
            peer: None,
        };
        let error = harden_generated_config_xml(
            "<configuration><gui><address>127.0.0.1:8384</address></gui></configuration>",
            &control,
            "100.124.210.13".parse().unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("missing required safety fields"));
    }

    #[test]
    fn local_idle_is_not_convergence_before_the_remote_index_arrives() {
        let local = serde_json::json!({
            "state": "idle",
            "needTotalItems": 0,
            "pullErrors": 0
        });

        assert!(!convergence_observation_is_complete(&local, None, "PEER"));
    }

    #[test]
    fn convergence_requires_both_local_and_remote_completion() {
        let local = serde_json::json!({
            "state": "idle",
            "needTotalItems": 0,
            "pullErrors": 0,
            "remoteSequence": { "PEER": 14 }
        });
        let remote = serde_json::json!({
            "remoteState": "valid",
            "needItems": 0,
            "needDeletes": 0,
            "needBytes": 0,
            "sequence": 14
        });

        assert!(convergence_observation_is_complete(
            &local,
            Some(&remote),
            "PEER"
        ));

        let still_receiving = serde_json::json!({
            "remoteState": "valid",
            "needItems": 1,
            "needDeletes": 0,
            "needBytes": 128,
            "sequence": 13
        });
        assert!(!convergence_observation_is_complete(
            &local,
            Some(&still_receiving),
            "PEER"
        ));
    }

    #[test]
    fn bilateral_completion_is_latched_for_the_peer_handoff() {
        let mut completion = CompletionLatch::default();
        for _ in 0..SYNC_COMPLETION_STABLE_OBSERVATIONS - 1 {
            assert!(!completion.observe(true));
        }
        assert!(completion.observe(true));
        assert!(completion.observe(false));
    }

    #[test]
    fn late_peer_index_activity_resets_completion_stability() {
        let mut completion = CompletionLatch::default();
        for _ in 0..SYNC_COMPLETION_STABLE_OBSERVATIONS - 1 {
            assert!(!completion.observe(true));
        }
        assert!(!completion.observe(false));
        for _ in 0..SYNC_COMPLETION_STABLE_OBSERVATIONS - 1 {
            assert!(!completion.observe(true));
        }
        assert!(completion.observe(true));
    }

    // Issue #101: after an interrupted transfer, Syncthing reports pull errors while the peer
    // reconnects and re-sends its index. Ending the run on the first one made recovery impossible.
    fn peer_complete() -> serde_json::Value {
        serde_json::json!({
            "remoteState": "valid",
            "needItems": 0,
            "needDeletes": 0,
            "needBytes": 0
        })
    }

    fn local_with_pull_errors(pull_errors: u64) -> serde_json::Value {
        serde_json::json!({
            "state": "idle",
            "needTotalItems": 0,
            "pullErrors": pull_errors,
            "remoteSequence": { "PEER": 14 }
        })
    }

    #[test]
    fn transient_pull_errors_do_not_end_a_run_that_later_converges() {
        let remote = peer_complete();
        let mut progress = SyncProgress::default();

        assert!(!progress.observe(&local_with_pull_errors(4569), Some(&remote), "PEER"));
        for _ in 0..SYNC_COMPLETION_STABLE_OBSERVATIONS - 1 {
            assert!(!progress.observe(&local_with_pull_errors(0), Some(&remote), "PEER"));
        }
        assert!(progress.observe(&local_with_pull_errors(0), Some(&remote), "PEER"));
    }

    #[test]
    fn a_pull_error_inside_a_clean_streak_restarts_the_stability_window() {
        let remote = peer_complete();
        let mut progress = SyncProgress::default();

        for _ in 0..SYNC_COMPLETION_STABLE_OBSERVATIONS - 1 {
            assert!(!progress.observe(&local_with_pull_errors(0), Some(&remote), "PEER"));
        }
        assert!(!progress.observe(&local_with_pull_errors(1), Some(&remote), "PEER"));
        for _ in 0..SYNC_COMPLETION_STABLE_OBSERVATIONS - 1 {
            assert!(!progress.observe(&local_with_pull_errors(0), Some(&remote), "PEER"));
        }
        assert!(progress.observe(&local_with_pull_errors(0), Some(&remote), "PEER"));
    }

    #[test]
    fn recent_pull_errors_name_the_deadline_failure() {
        let still_scanning = serde_json::json!({ "state": "scanning" });
        let mut progress = SyncProgress::default();
        assert_eq!(
            progress.failure(),
            "Sync did not converge within five minutes"
        );

        progress.observe(&local_with_pull_errors(2), None, "PEER");
        assert_eq!(
            progress.failure(),
            "Syncthing reported one or more file errors"
        );

        // Syncthing can report zero errors between its once-a-minute retries.
        for _ in 0..PULL_ERROR_EXPLAINS_FAILURE - 1 {
            progress.observe(&still_scanning, None, "PEER");
        }
        assert_eq!(
            progress.failure(),
            "Syncthing reported one or more file errors"
        );

        progress.observe(&still_scanning, None, "PEER");
        assert_eq!(
            progress.failure(),
            "Sync did not converge within five minutes"
        );
    }
}
