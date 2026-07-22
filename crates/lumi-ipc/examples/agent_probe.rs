use lumi_ipc::{default_pipe_name, AgentCommand, NamedPipeClient};
use std::error::Error;

fn command(argument: Option<&str>) -> Result<AgentCommand, String> {
    Ok(match argument.unwrap_or("snapshot") {
        "snapshot" => AgentCommand::GetSnapshot,
        "settings" => AgentCommand::GetSettings,
        "ping" => AgentCommand::Ping,
        "run" => AgentCommand::RunNow,
        "refresh" => AgentCommand::RefreshHardware,
        "diagnostics" => AgentCommand::ExportDiagnostics,
        "open" => AgentCommand::OpenUi,
        "shutdown" => AgentCommand::Shutdown,
        "pause" => AgentCommand::SetPaused { paused: true },
        "resume" => AgentCommand::SetPaused { paused: false },
        "light-on" => AgentCommand::SetLight { light_on: true },
        "light-off" => AgentCommand::SetLight { light_on: false },
        "--help" | "-h" => {
            println!(
                "Usage: cargo run -p lumi-ipc --example agent_probe -- [snapshot|settings|ping|run|refresh|diagnostics|open|shutdown|pause|resume|light-on|light-off]"
            );
            std::process::exit(0);
        }
        other => return Err(format!("unknown command: {other}")),
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Agent probe failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let requested = command(std::env::args().nth(1).as_deref())?;
    let mut client = NamedPipeClient::connect(&default_pipe_name()?, 2_000)?;
    let response = client.call(requested)?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    if let Some(error) = response.error {
        return Err(format!("{:?}: {}", error.code, error.message).into());
    }
    Ok(())
}
