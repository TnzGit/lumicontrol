use lumi_ipc::{
    default_pipe_name, AgentCommand, AgentSnapshot, IpcResponse, NamedPipeClient, ResponsePayload,
};
use lumi_store::SettingsDocument;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{LogicalSize, Manager, Size};

const CONNECT_TIMEOUT_MS: u32 = 1_500;
const WINDOW_WIDTH: f64 = 680.0;
const COMPACT_WINDOW_HEIGHT: f64 = 352.0;
const ONBOARDING_WINDOW_HEIGHT: f64 = 520.0;
const _: () = assert!(ONBOARDING_WINDOW_HEIGHT > COMPACT_WINDOW_HEIGHT);
static AGENT_START_LOCK: Mutex<()> = Mutex::new(());

mod app_updates {
    use super::*;
    use std::sync::Mutex;
    use tauri::{AppHandle, State};
    use tauri_plugin_updater::{Update, UpdaterExt};
    use url::Url;

    #[derive(Default)]
    pub(super) struct PendingUpdate(pub Mutex<Option<Update>>);

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct UpdateChannelStatus {
        configured: bool,
        current_version: String,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct UpdateMetadata {
        version: String,
        current_version: String,
        notes: Option<String>,
    }

    fn configuration() -> Result<(Url, &'static str), String> {
        let endpoint = option_env!("LUMICONTROL_UPDATE_ENDPOINT")
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Update channel is not configured for this build".to_string())?;
        let public_key = option_env!("LUMICONTROL_UPDATE_PUBKEY")
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Update signing key is not configured for this build".to_string())?;
        let endpoint =
            Url::parse(endpoint).map_err(|error| format!("Update endpoint is invalid: {error}"))?;
        if endpoint.scheme() != "https" {
            return Err("Update endpoint must use HTTPS".to_string());
        }
        Ok((endpoint, public_key))
    }

    #[tauri::command]
    pub(super) fn update_channel_status() -> UpdateChannelStatus {
        UpdateChannelStatus {
            configured: configuration().is_ok(),
            current_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    #[tauri::command]
    pub(super) async fn check_for_update(
        app: AppHandle,
        pending: State<'_, PendingUpdate>,
    ) -> Result<Option<UpdateMetadata>, String> {
        let (endpoint, public_key) = configuration()?;
        let update = app
            .updater_builder()
            .endpoints(vec![endpoint])
            .map_err(|error| error.to_string())?
            .pubkey(public_key)
            .timeout(Duration::from_secs(15))
            .on_before_exit(shutdown_agent_for_update)
            .build()
            .map_err(|error| error.to_string())?
            .check()
            .await
            .map_err(|error| error.to_string())?;
        let metadata = update.as_ref().map(|update| UpdateMetadata {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            notes: update.body.clone(),
        });
        *pending
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = update;
        Ok(metadata)
    }

    #[tauri::command]
    pub(super) async fn install_update(pending: State<'_, PendingUpdate>) -> Result<(), String> {
        let update = pending
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| "There is no checked update to install".to_string())?;
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|error| error.to_string())
    }
}

fn shutdown_agent_for_update() {
    let _ = call_agent_direct(AgentCommand::Shutdown, CONNECT_TIMEOUT_MS);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if call_agent_direct(AgentCommand::Ping, 150).is_err() {
            break;
        }
        thread::sleep(Duration::from_millis(75));
    }
}

#[derive(Debug)]
enum AgentCallError {
    Transport(String),
    Remote(String),
}

impl AgentCallError {
    fn into_message(self) -> String {
        match self {
            Self::Transport(message) | Self::Remote(message) => message,
        }
    }
}

fn response_payload(response: IpcResponse) -> Result<ResponsePayload, AgentCallError> {
    if let Some(error) = response.error {
        return Err(AgentCallError::Remote(format!(
            "{}: {}",
            error_code_name(error.code),
            error.message
        )));
    }
    response
        .result
        .ok_or_else(|| AgentCallError::Remote("Agent returned an empty response".to_string()))
}

fn error_code_name(code: lumi_ipc::IpcErrorCode) -> &'static str {
    use lumi_ipc::IpcErrorCode;
    match code {
        IpcErrorCode::InvalidRequest => "invalid_request",
        IpcErrorCode::IncompatibleApi => "incompatible_api",
        IpcErrorCode::InvalidSettings => "invalid_settings",
        IpcErrorCode::UnsupportedCapability => "unsupported_capability",
        IpcErrorCode::HardwareUnavailable => "hardware_unavailable",
        IpcErrorCode::Timeout => "timeout",
        IpcErrorCode::Internal => "internal",
    }
}

fn call_agent_direct(
    command: AgentCommand,
    timeout_ms: u32,
) -> Result<ResponsePayload, AgentCallError> {
    let pipe_name =
        default_pipe_name().map_err(|error| AgentCallError::Transport(error.to_string()))?;
    let mut client = NamedPipeClient::connect(&pipe_name, timeout_ms)
        .map_err(|error| AgentCallError::Transport(error.to_string()))?;
    let response = client
        .call(command)
        .map_err(|error| AgentCallError::Transport(error.to_string()))?;
    response_payload(response)
}

fn agent_executable() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = executable
        .parent()
        .ok_or_else(|| "UI executable has no parent directory".to_string())?;
    Ok(directory.join(if cfg!(windows) {
        "lumi-agent.exe"
    } else {
        "lumi-agent"
    }))
}

fn ensure_agent_running() -> Result<(), String> {
    match call_agent_direct(AgentCommand::Ping, 200) {
        Ok(_) => return Ok(()),
        Err(AgentCallError::Remote(message)) => return Err(message),
        Err(AgentCallError::Transport(_)) => {}
    }

    let _start_guard = AGENT_START_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match call_agent_direct(AgentCommand::Ping, 200) {
        Ok(_) => return Ok(()),
        Err(AgentCallError::Remote(message)) => return Err(message),
        Err(AgentCallError::Transport(_)) => {}
    }

    let agent = agent_executable()?;
    if !agent.is_file() {
        return Err(format!(
            "Lumi Agent is not installed at {}",
            agent.display()
        ));
    }
    Command::new(&agent)
        .arg("--background")
        .spawn()
        .map_err(|error| format!("Could not start Lumi Agent: {error}"))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match call_agent_direct(AgentCommand::Ping, 250) {
            Ok(_) => return Ok(()),
            Err(AgentCallError::Remote(message)) => return Err(message),
            Err(AgentCallError::Transport(_)) => {}
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("Lumi Agent did not become ready within five seconds".to_string())
}

fn call_agent(command: AgentCommand, timeout_ms: u32) -> Result<ResponsePayload, String> {
    match call_agent_direct(command.clone(), timeout_ms) {
        Ok(payload) => return Ok(payload),
        Err(AgentCallError::Remote(message)) => return Err(message),
        Err(AgentCallError::Transport(_)) => {}
    }
    ensure_agent_running()?;
    call_agent_direct(command, timeout_ms).map_err(AgentCallError::into_message)
}

async fn execute(command: AgentCommand, timeout_ms: u32) -> Result<ResponsePayload, String> {
    tauri::async_runtime::spawn_blocking(move || call_agent(command, timeout_ms))
        .await
        .map_err(|error| format!("UI worker stopped: {error}"))?
}

#[tauri::command]
async fn get_snapshot() -> Result<AgentSnapshot, String> {
    match execute(AgentCommand::GetSnapshot, CONNECT_TIMEOUT_MS).await? {
        ResponsePayload::Snapshot(snapshot) => Ok(*snapshot),
        _ => Err("Agent returned the wrong payload for get_snapshot".to_string()),
    }
}

#[tauri::command(rename_all = "camelCase")]
async fn wait_for_snapshot(after_revision: u64, timeout_ms: u32) -> Result<AgentSnapshot, String> {
    match execute(
        AgentCommand::WaitForSnapshot {
            after_revision,
            timeout_ms,
        },
        timeout_ms.saturating_add(CONNECT_TIMEOUT_MS),
    )
    .await?
    {
        ResponsePayload::Snapshot(snapshot) => Ok(*snapshot),
        _ => Err("Agent returned the wrong payload for wait_for_snapshot".to_string()),
    }
}

#[tauri::command]
async fn get_settings() -> Result<SettingsDocument, String> {
    match execute(AgentCommand::GetSettings, CONNECT_TIMEOUT_MS).await? {
        ResponsePayload::Settings(settings) => Ok(*settings),
        _ => Err("Agent returned the wrong payload for get_settings".to_string()),
    }
}

#[tauri::command]
async fn save_settings(document: SettingsDocument) -> Result<(), String> {
    match execute(
        AgentCommand::SaveSettings {
            document: Box::new(document),
        },
        CONNECT_TIMEOUT_MS,
    )
    .await?
    {
        ResponsePayload::Acknowledged => Ok(()),
        _ => Err("Agent returned the wrong payload for save_settings".to_string()),
    }
}

#[tauri::command]
async fn set_paused(paused: bool) -> Result<(), String> {
    acknowledge(AgentCommand::SetPaused { paused }).await
}

#[tauri::command]
async fn run_now() -> Result<(), String> {
    acknowledge(AgentCommand::RunNow).await
}

#[tauri::command]
async fn refresh_hardware() -> Result<(), String> {
    acknowledge(AgentCommand::RefreshHardware).await
}

#[tauri::command(rename_all = "camelCase")]
async fn set_light(light_on: bool) -> Result<(), String> {
    acknowledge(AgentCommand::SetLight { light_on }).await
}

#[tauri::command(rename_all = "camelCase")]
async fn clear_manual_override(monitor_id: Option<String>) -> Result<(), String> {
    acknowledge(AgentCommand::ClearManualOverride { monitor_id }).await
}

#[tauri::command]
async fn export_diagnostics() -> Result<String, String> {
    match execute(AgentCommand::ExportDiagnostics, 10_000).await? {
        ResponsePayload::DiagnosticsExported { path } => Ok(path),
        _ => Err("Agent returned the wrong payload for export_diagnostics".to_string()),
    }
}

#[tauri::command(rename_all = "camelCase")]
fn set_window_mode(window: tauri::WebviewWindow, onboarding: bool) -> Result<(), String> {
    let height = if onboarding {
        ONBOARDING_WINDOW_HEIGHT
    } else {
        COMPACT_WINDOW_HEIGHT
    };
    window.unminimize().map_err(|error| error.to_string())?;
    window
        .set_size(Size::Logical(LogicalSize::new(WINDOW_WIDTH, height)))
        .map_err(|error| error.to_string())?;
    window.center().map_err(|error| error.to_string())
}

async fn acknowledge(command: AgentCommand) -> Result<(), String> {
    match execute(command, CONNECT_TIMEOUT_MS).await? {
        ResponsePayload::Acknowledged => Ok(()),
        _ => Err("Agent did not acknowledge the command".to_string()),
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(app_updates::PendingUpdate::default())
        .setup(|app| {
            let window = app
                .get_webview_window("main")
                .ok_or("main window was not created")?;
            window.set_size(Size::Logical(LogicalSize::new(
                WINDOW_WIDTH,
                COMPACT_WINDOW_HEIGHT,
            )))?;
            window.center()?;
            window.unminimize()?;
            window.show()?;
            window.set_focus()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            wait_for_snapshot,
            get_settings,
            save_settings,
            set_paused,
            run_now,
            refresh_hardware,
            set_light,
            clear_manual_override,
            export_diagnostics,
            set_window_mode,
            app_updates::update_channel_status,
            app_updates::check_for_update,
            app_updates::install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running LumiControl UI");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_binary_is_expected_next_to_ui() {
        let path = agent_executable().expect("current executable path");
        assert_eq!(
            path.file_stem().and_then(|name| name.to_str()),
            Some("lumi-agent")
        );
    }

    #[test]
    fn onboarding_and_compact_window_heights_are_distinct() {
        assert_eq!(COMPACT_WINDOW_HEIGHT, 352.0);
        assert_eq!(ONBOARDING_WINDOW_HEIGHT, 520.0);
    }
}
