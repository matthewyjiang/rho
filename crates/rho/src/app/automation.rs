use std::{
    fmt,
    io::{self, Read, Write},
    path::PathBuf,
    sync::Arc,
};

use rho_sdk::{SessionOptions, SystemPrompt, UserInput, Workspace};

use {
    crate::agent::PromptPolicy,
    crate::cli::Command,
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::{HerdrReporter, HerdrState},
    crate::prompt,
    crate::subagent::{self, RunState, RunStatus},
    crate::tools::sdk_registry::{AppToolSet, ToolSetOptions},
    crate::tui::AttachmentWriter,
    rho_providers::credentials::OsCredentialStore,
    rho_providers::providers::build_automation_provider,
};

use super::{
    agent_binding::BoundAgent,
    policy::AppPolicy,
    runtime_builder::{build_runtime, configured_context_window, RuntimeBuildOptions},
    sdk_config::SdkBootstrapOptions,
};

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

#[derive(Debug)]
struct SubagentCancelled;

impl fmt::Display for SubagentCancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("subagent cancellation requested")
    }
}

impl std::error::Error for SubagentCancelled {}

pub(super) struct Startup<'a> {
    pub config: &'a Config,
    pub config_path: PathBuf,
    pub cwd: PathBuf,
    pub no_system_prompt: bool,
    pub no_tools: bool,
    pub no_subagents: bool,
    pub usage_purpose: &'static str,
    pub parent_session_id: Option<rho_sdk::SessionId>,
    pub agent: BoundAgent,
    pub output_file: Option<PathBuf>,
    pub diagnostics: RuntimeDiagnostics,
    pub herdr: HerdrReporter,
}

pub(super) fn prompt_for_command(command: &Option<Command>) -> anyhow::Result<Option<String>> {
    match command {
        Some(Command::Run { prompt, stdin, .. }) => {
            prompt_from_stdin(prompt.clone(), *stdin).map(Some)
        }
        Some(Command::Attach { .. } | Command::Login { .. } | Command::Update) | None => Ok(None),
    }
}

pub(super) async fn run(prompt_text: String, startup: Startup<'_>) -> anyhow::Result<()> {
    // The reporter exists before anything that can fail, so a parent process
    // watching the output file always sees a terminal state — even when the
    // run dies during startup (bad auth, broken workspace, ...).
    let mut reporter = startup
        .output_file
        .as_ref()
        .map(|path| {
            RunReporter::new(
                path.clone(),
                RunArtifactIdentity {
                    agent_id: startup.agent.id().to_string(),
                    agent_fingerprint: startup.agent.fingerprint().to_string(),
                    provider: startup.config.provider.clone(),
                    model: startup.config.model.clone(),
                },
                startup.cwd.clone(),
                &prompt_text,
                /* stream_output */ true,
                None,
            )
        })
        .transpose()?;
    let result = run_session(prompt_text, &startup, reporter.as_mut(), None).await;
    if let Some(reporter) = reporter.as_mut() {
        reporter.finish(&result);
    }
    let answer = result?;
    let mut stdout = io::stdout().lock();
    if reporter.is_some() {
        // The answer already streamed above and is in the result file.
        writeln!(stdout, "\n[subagent run complete]")?;
    } else {
        writeln!(stdout, "{}", answer.text())?;
    }
    stdout.flush()?;
    Ok(())
}

pub(crate) async fn run_session(
    prompt_text: String,
    startup: &Startup<'_>,
    reporter: Option<&mut RunReporter>,
    cancellation: Option<rho_tools::cancellation::RunCancellation>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    let sdk_options = SdkBootstrapOptions::from_config(startup.config, &startup.cwd)?;
    let credentials = rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(OsCredentialStore),
    );
    let provider = build_automation_provider(sdk_options.provider, &credentials)?;
    let delegation_available = startup.config.enable_subagents && !startup.no_subagents;
    let launch_delegation_enabled = delegation_available && startup.agent.tools().contains("agent");
    let delegation_enabled = launch_delegation_enabled
        || (delegation_available && startup.agent.tools().contains("agents"));
    let mut tool_set = if startup.no_tools {
        AppToolSet::disabled()
    } else {
        let delegation_cwd = delegation_enabled.then(|| startup.cwd.clone());
        AppToolSet::new(
            startup.config,
            startup.diagnostics.clone(),
            ToolSetOptions::default()
                .delegation_tools(delegation_cwd, startup.agent.tools())
                .subagent_config_path(startup.config_path.clone()),
        )
    };
    let allowed = startup.agent.tools().iter().cloned().collect::<Vec<_>>();
    tool_set.retain_named(&allowed);
    let tool_specs = tool_set.specs();
    let system_prompt = if startup.no_system_prompt {
        startup.diagnostics.update_prompt_sources(Vec::new());
        SystemPrompt::None
    } else {
        let mut text = match startup.agent.prompt() {
            PromptPolicy::Replace(text) => text.clone(),
            PromptPolicy::Extend(extra) => {
                let built = prompt::system_prompt(&tool_specs, &startup.cwd);
                startup.diagnostics.update_prompt_sources(built.sources);
                let mut text = built.text;
                if !launch_delegation_enabled {
                    prompt::append_subagents_disabled_instruction(&mut text);
                }
                if !extra.is_empty() {
                    text.push_str("\n\n# Agent instructions\n\n");
                    text.push_str(extra);
                }
                text
            }
        };
        if text.is_empty() {
            text = "You are a coding agent.".into();
        }
        SystemPrompt::Custom(text)
    };
    startup.diagnostics.update_tools(&tool_specs);

    let workspace = Workspace::new(&sdk_options.workspace.root)?;
    let context_window = configured_context_window(startup.config);
    let compaction = sdk_options.runtime.compaction.clone();
    startup.diagnostics.update_compaction_config(&compaction);
    let usage_recording = crate::usage::default_recording().await;
    let runtime = build_runtime(RuntimeBuildOptions {
        provider,
        tools: tool_set.tools(),
        workspace,
        workspace_policy: AppPolicy::for_mode(startup.config.permission_mode),
        approval_handler: None,
        system_prompt,
        reasoning: sdk_options.runtime.reasoning,
        compaction,
        context_window,
        usage_purpose: startup.usage_purpose,
        usage_parent_session_id: startup.parent_session_id.clone(),
        usage_recording,
    })?;
    let session = runtime.session(SessionOptions::default()).await?;
    if let Some(manager) = tool_set.subagents() {
        manager.set_session(session.id().to_string());
    }

    startup
        .herdr
        .report_state(HerdrState::Working, None, None)
        .await;
    let result = complete_run(&session, prompt_text, reporter, cancellation).await;

    runtime.shutdown();
    tool_set.shutdown().await;
    startup
        .herdr
        .report_state(HerdrState::Idle, None, None)
        .await;
    startup.herdr.release().await;

    result
}

async fn complete_run(
    session: &rho_sdk::Session,
    prompt_text: String,
    reporter: Option<&mut RunReporter>,
    external_cancellation: Option<rho_tools::cancellation::RunCancellation>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    let mut run = session.start(UserInput::text(prompt_text)).await?;
    let cancellation = run.cancellation_handle();
    let external_cancellation = external_cancellation.unwrap_or_default();
    tokio::select! {
        outcome = drive_headless_run(&mut run, reporter) => outcome,
        signal = shutdown_signal() => {
            let signal = signal?;
            cancellation.cancel();
            let _ = run.outcome().await;
            Err(AutomationInterrupted::new(signal).into())
        }
        () = external_cancellation.cancelled() => {
            cancellation.cancel();
            let _ = run.outcome().await;
            Err(SubagentCancelled.into())
        }
    }
}

/// Drains run events with no interactive host attached.
///
/// Host input requests cannot be answered headlessly; cancel instead of
/// leaving the requesting tool suspended until a signal arrives.
async fn drive_headless_run(
    run: &mut rho_sdk::Run,
    mut reporter: Option<&mut RunReporter>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    let mut heartbeat = tokio::time::interval(REPORT_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let event = tokio::select! {
            event = run.next_event() => event,
            _ = heartbeat.tick(), if reporter.is_some() => {
                if let Some(reporter) = reporter.as_deref_mut() {
                    reporter.write();
                }
                continue;
            }
        };
        let Some(event) = event else {
            break;
        };
        if let Some(reporter) = reporter.as_deref_mut() {
            reporter.on_event(&event);
        }
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

pub(crate) struct RunArtifactIdentity {
    pub(crate) agent_id: String,
    pub(crate) agent_fingerprint: String,
    pub(crate) provider: String,
    pub(crate) model: String,
}

/// Maintains the `--output-file` status contract for subagent runs and
/// streams progress to stdout so a watching pane shows live activity.
pub(crate) struct RunReporter {
    path: PathBuf,
    status: RunStatus,
    attachment: Option<AttachmentWriter>,
    stream_output: bool,
    status_tx: Option<tokio::sync::watch::Sender<RunStatus>>,
    last_write: std::time::Instant,
}

/// Longest a status-file write is deferred while text streams.
const REPORT_THROTTLE: std::time::Duration = std::time::Duration::from_secs(2);
/// Keeps the status file fresh while a provider or tool call emits no events.
const REPORT_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);
const LAST_TEXT_BYTES: usize = 400;

impl RunReporter {
    pub(crate) fn new(
        path: PathBuf,
        identity: RunArtifactIdentity,
        cwd: PathBuf,
        prompt: &str,
        stream_output: bool,
        status_tx: Option<tokio::sync::watch::Sender<RunStatus>>,
    ) -> anyhow::Result<Self> {
        let status = RunStatus {
            state: RunState::Starting,
            agent_id: Some(identity.agent_id),
            agent_fingerprint: Some(identity.agent_fingerprint),
            provider: Some(identity.provider),
            model: Some(identity.model),
            ..RunStatus::default()
        };
        subagent::write_status(&path, &status)?;
        let attachment = match AttachmentWriter::new(&path, cwd, prompt) {
            Ok(attachment) => Some(attachment),
            Err(error) => {
                let mut status = status;
                status.attachment_error = Some(format!("could not record attach output: {error}"));
                subagent::write_status(&path, &status)?;
                return Ok(Self {
                    path,
                    status,
                    attachment: None,
                    stream_output,
                    status_tx,
                    last_write: std::time::Instant::now(),
                });
            }
        };
        Ok(Self {
            path,
            status,
            attachment,
            stream_output,
            status_tx,
            last_write: std::time::Instant::now(),
        })
    }

    fn on_event(&mut self, event: &rho_sdk::RunEvent) {
        use rho_sdk::RunEvent;

        if let Some(attachment) = self.attachment.as_mut() {
            if let Err(error) = attachment.on_event(event) {
                self.status.attachment_error =
                    Some(format!("could not record attach output: {error}"));
                self.attachment = None;
                self.write();
            }
        }
        match event {
            RunEvent::StepStarted { step } => {
                self.status.state = RunState::Running;
                self.status.turns = *step as u64;
                self.write();
            }
            RunEvent::ToolStarted { name, .. } => {
                self.status.last_activity = Some(format!("tool: {name}"));
                self.stream(&format!("\n[tool] {name}\n"));
                self.write();
            }
            RunEvent::AssistantTextDelta { text } => {
                self.status.last_activity = Some("assistant text".into());
                append_tail(
                    self.status.last_text.get_or_insert_with(String::new),
                    text,
                    LAST_TEXT_BYTES,
                );
                self.stream(text);
                self.write_throttled();
            }
            RunEvent::UsageUpdated { usage } => {
                self.status.input_tokens = usage.total_input_tokens().unwrap_or(0);
                self.status.output_tokens = usage.output_tokens.unwrap_or(0);
            }
            _ => {}
        }
    }

    pub(crate) fn finish(&mut self, result: &anyhow::Result<rho_sdk::RunOutcome>) {
        match result {
            Ok(outcome) => {
                self.status.state = RunState::Ok;
                self.status.result = Some(outcome.text().to_string());
                let usage = outcome.usage();
                self.status.input_tokens = usage.total_input_tokens().unwrap_or(0);
                self.status.output_tokens = usage.output_tokens.unwrap_or(0);
            }
            Err(error)
                if error.is::<AutomationInterrupted>() || error.is::<SubagentCancelled>() =>
            {
                self.status.state = RunState::Stopped;
                self.status.result = self
                    .status
                    .last_text
                    .as_ref()
                    .map(|text| format!("(partial, stopped before finishing)\n{text}"));
            }
            Err(error) => {
                self.status.state = RunState::Error;
                self.status.error = Some(format!("{error:#}"));
            }
        }
        self.write();
    }

    fn stream(&self, text: &str) {
        if !self.stream_output {
            return;
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }

    fn write_throttled(&mut self) {
        if self.last_write.elapsed() >= REPORT_THROTTLE {
            self.write();
        }
    }

    fn write(&mut self) {
        self.last_write = std::time::Instant::now();
        if let Some(status_tx) = &self.status_tx {
            status_tx.send_replace(self.status.clone());
        }
        let _ = subagent::write_status(&self.path, &self.status);
    }
}

/// Appends to a rolling tail buffer capped at `max` bytes.
fn append_tail(buffer: &mut String, text: &str, max: usize) {
    buffer.push_str(text);
    if buffer.len() > max {
        let cut = buffer.len() - max;
        let boundary = (cut..buffer.len())
            .find(|index| buffer.is_char_boundary(*index))
            .unwrap_or(buffer.len());
        buffer.drain(..boundary);
    }
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
