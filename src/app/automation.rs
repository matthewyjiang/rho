use std::{
    fmt,
    io::{self, Read, Write},
    path::PathBuf,
    sync::Arc,
};

use rho_sdk::{
    CapabilityRequest, PolicyDecision, Rho, SessionOptions, SystemPrompt, UserInput, Workspace,
    WorkspacePolicy,
};

use crate::{
    cli::Command,
    config::Config,
    credentials::OsCredentialStore,
    diagnostics::RuntimeDiagnostics,
    herdr::{HerdrReporter, HerdrState},
    prompt,
    providers::build_automation_provider,
    tools::sdk_registry::AutomationToolSet,
};

use super::sdk_config::SdkBootstrapOptions;

/// Error returned after an automation run handles an interrupt and completes cleanup.
#[derive(Debug)]
pub struct AutomationInterrupted {
    signal: ShutdownSignal,
}

impl AutomationInterrupted {
    fn new(signal: ShutdownSignal) -> Self {
        Self { signal }
    }

    /// Returns the conventional process exit code for the received signal.
    pub fn exit_code(&self) -> u8 {
        match self.signal {
            ShutdownSignal::Interrupt => 130,
            ShutdownSignal::Terminate => 143,
        }
    }
}

impl fmt::Display for AutomationInterrupted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "rho run interrupted by {}", self.signal)
    }
}

impl std::error::Error for AutomationInterrupted {}

#[derive(Clone, Copy, Debug)]
enum ShutdownSignal {
    Interrupt,
    Terminate,
}

impl fmt::Display for ShutdownSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupt => formatter.write_str("SIGINT"),
            Self::Terminate => formatter.write_str("SIGTERM"),
        }
    }
}

pub(super) struct Startup<'a> {
    pub config: &'a Config,
    pub cwd: PathBuf,
    pub no_system_prompt: bool,
    pub no_tools: bool,
    pub diagnostics: RuntimeDiagnostics,
    pub herdr: HerdrReporter,
}

pub(super) fn prompt_for_command(command: &Option<Command>) -> anyhow::Result<Option<String>> {
    match command {
        Some(Command::Run { prompt, stdin }) => prompt_from_stdin(prompt.clone(), *stdin).map(Some),
        Some(Command::Login { .. }) | Some(Command::Update) | None => Ok(None),
    }
}

pub(super) async fn run(prompt_text: String, startup: Startup<'_>) -> anyhow::Result<()> {
    let sdk_options = SdkBootstrapOptions::from_config(startup.config, &startup.cwd)?;
    let credentials = crate::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(OsCredentialStore),
    );
    let provider = build_automation_provider(sdk_options.provider, &credentials)?;
    let tool_set = if startup.no_tools {
        AutomationToolSet::disabled()
    } else {
        AutomationToolSet::enabled(startup.config, startup.diagnostics.clone())
    };
    let tool_specs = tool_set.specs();
    let system_prompt = if startup.no_system_prompt {
        startup.diagnostics.update_prompt_sources(Vec::new());
        SystemPrompt::None
    } else {
        let system_prompt = prompt::system_prompt(&tool_specs, &startup.cwd);
        startup
            .diagnostics
            .update_prompt_sources(system_prompt.sources);
        SystemPrompt::Custom(system_prompt.text)
    };
    startup.diagnostics.update_tools(&tool_specs);

    let workspace = Workspace::new(&sdk_options.workspace.root)?;
    let mut builder = Rho::builder()
        .provider_shared(provider)
        .system_prompt(system_prompt)
        .workspace(workspace)
        .workspace_policy(AutomationWorkspacePolicy)
        .max_steps(super::sdk_config::run_step_limit())
        .reasoning_level(sdk_options.runtime.reasoning);
    for tool in tool_set.tools() {
        builder = builder.tool_shared(tool.clone());
    }
    let runtime = builder.build()?;
    let session = runtime.session(SessionOptions::default()).await?;

    startup
        .herdr
        .report_state(HerdrState::Working, None, None)
        .await;
    let result = complete_run(&session, prompt_text).await;

    runtime.shutdown();
    tool_set.shutdown().await;
    startup
        .herdr
        .report_state(HerdrState::Idle, None, None)
        .await;
    startup.herdr.release().await;

    let answer = result?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{}", answer.text())?;
    stdout.flush()?;
    Ok(())
}

async fn complete_run(
    session: &rho_sdk::Session,
    prompt_text: String,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    let mut run = session.start(UserInput::text(prompt_text)).await?;
    let cancellation = run.cancellation_handle();
    tokio::select! {
        outcome = drive_headless_run(&mut run) => outcome,
        signal = shutdown_signal() => {
            let signal = signal?;
            cancellation.cancel();
            let _ = run.outcome().await;
            Err(AutomationInterrupted::new(signal).into())
        }
    }
}

/// Drains run events with no interactive host attached.
///
/// Host input requests cannot be answered headlessly; cancel instead of
/// leaving the requesting tool suspended until a signal arrives.
async fn drive_headless_run(run: &mut rho_sdk::Run) -> anyhow::Result<rho_sdk::RunOutcome> {
    while let Some(event) = run.next_event().await {
        if let rho_sdk::RunEvent::HostInputRequested { request } = event {
            run.cancel();
            let _ = run.outcome().await;
            anyhow::bail!(
                "rho run cannot answer host input request '{}' ({}); run without tools that require interactive input",
                request.id(),
                request.title(),
            );
        }
    }
    Ok(run.outcome().await?)
}

#[cfg(unix)]
async fn shutdown_signal() -> io::Result<ShutdownSignal> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => Ok(ShutdownSignal::Interrupt),
        _ = terminate.recv() => Ok(ShutdownSignal::Terminate),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> io::Result<ShutdownSignal> {
    tokio::signal::ctrl_c().await?;
    Ok(ShutdownSignal::Interrupt)
}

#[derive(Clone, Copy, Debug)]
struct AutomationWorkspacePolicy;

impl WorkspacePolicy for AutomationWorkspacePolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

fn prompt_from_stdin(parts: Vec<String>, read_stdin: bool) -> anyhow::Result<String> {
    prompt_from_reader(parts, read_stdin, &mut io::stdin())
}

fn prompt_from_reader(
    parts: Vec<String>,
    read_stdin: bool,
    stdin: &mut impl Read,
) -> anyhow::Result<String> {
    let mut chunks = Vec::new();
    let inline = parts.join(" ").trim().to_string();
    if !inline.is_empty() {
        chunks.push(inline);
    }
    if read_stdin {
        let mut buffer = String::new();
        stdin.read_to_string(&mut buffer)?;
        let buffer = buffer.trim().to_string();
        if !buffer.is_empty() {
            chunks.push(buffer);
        }
    }

    let prompt = chunks.join("\n\n");
    if prompt.is_empty() {
        anyhow::bail!("rho run requires a prompt argument or --stdin");
    }
    Ok(prompt)
}

#[cfg(test)]
#[path = "automation_tests.rs"]
mod tests;
