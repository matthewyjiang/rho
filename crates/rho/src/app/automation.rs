use std::{
    fmt,
    io::{self, Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use rho_sdk::UserInput;

use {
    crate::agent::PERMISSION_CLASSIFIER_AGENT_ID,
    crate::cli::{Command, OutputFormat},
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::{HerdrReporter, HerdrState},
    crate::permission::{PermissionMode, SessionWriteLog},
    crate::permission_classifier_handler::ClassifierApprovalHandler,
    crate::subagent::{RunState, RunStatus},
    crate::tools::agent::BackgroundSubagents,
};

use super::{
    agent_binding::BoundAgent,
    automation_protocol::{write_event, JsonlAdapter, TerminalReason, WireEvent},
    headless_run::{self, HeadlessRunDeps, HostInputResponder},
    session_assembly::{
        assemble_session, ApprovalInputs, SessionApproval, SessionAssembly, SessionAssemblyOptions,
    },
};

/// Error returned after an automation run has cleaned up and selected a stable exit code.
#[derive(Debug)]
pub struct AutomationExit {
    code: u8,
    reason: TerminalReason,
    message: String,
}

impl AutomationExit {
    pub(super) fn new(code: u8, reason: TerminalReason, message: impl Into<String>) -> Self {
        Self {
            code,
            reason,
            message: message.into(),
        }
    }

    /// Returns the documented process exit code for this automation result.
    pub fn exit_code(&self) -> u8 {
        self.code
    }

    fn reason(&self) -> TerminalReason {
        self.reason
    }
}

impl fmt::Display for AutomationExit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AutomationExit {}

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
        self.signal.exit_code()
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

impl ShutdownSignal {
    fn exit_code(self) -> u8 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }
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
    pub output: OutputFormat,
    pub max_steps: Option<NonZeroUsize>,
    pub timeout: Option<Duration>,
    pub diagnostics: RuntimeDiagnostics,
    pub herdr: HerdrReporter,
    pub host_input: Option<Arc<dyn HostInputResponder>>,
    /// Non-blocking parent notices for background delegated Rho agents.
    pub notice_poster: Option<Arc<dyn super::subagent_messaging::NoticePoster>>,
    /// Receives the live steering port once the Rho session starts.
    pub steering_slot: Option<super::subagent_messaging::SteeringSlot>,
    pub approval_session: Option<rho_sdk::ApprovalSession>,
    pub approval_classifier: Option<Arc<ClassifierApprovalHandler>>,
    pub hook_host_labels: rho_sdk::hooks::HookHostLabels,
}

pub(super) fn prompt_for_command(command: &Option<Command>) -> anyhow::Result<Option<String>> {
    match command {
        Some(Command::Run { prompt, stdin, .. }) => {
            prompt_from_stdin(prompt.clone(), *stdin).map(Some)
        }
        Some(
            Command::Attach { .. }
            | Command::Login { .. }
            | Command::CredentialStore { .. }
            | Command::Sessions { .. }
            | Command::Mcp { .. }
            | Command::Plugins { .. }
            | Command::Workflow { .. }
            | Command::WorkflowPlannerWorker
            | Command::Update
            | Command::Acp,
        )
        | None => Ok(None),
    }
}

pub(super) fn emit_startup_failure(message: impl Into<String>) -> anyhow::Result<()> {
    let mut adapter = JsonlAdapter::new();
    let event = adapter.failed(TerminalReason::ConfigurationError, message.into(), None);
    emit(event)
}

pub(super) async fn run(prompt_text: String, startup: Startup<'_>) -> anyhow::Result<()> {
    let mut jsonl = (startup.output == OutputFormat::Jsonl).then(JsonlAdapter::new);
    let deadline = startup
        .timeout
        .map(|timeout| tokio::time::Instant::now() + timeout);
    // The reporter exists before anything that can fail, so a parent process
    // watching the output file always sees a terminal state, including startup failures.
    let reporter_result = startup
        .output_file
        .as_ref()
        .map(|path| {
            RunReporter::new(
                path.clone(),
                startup.agent.artifact_identity(),
                startup.cwd.clone(),
                &prompt_text,
                /* stream_output */ startup.output == OutputFormat::Text,
                None,
            )
        })
        .transpose();
    let mut reporter = match reporter_result {
        Ok(reporter) => reporter,
        Err(error) => {
            emit_failure(&mut jsonl, TerminalReason::OutputError, &error)?;
            return Err(
                AutomationExit::new(1, TerminalReason::OutputError, error.to_string()).into(),
            );
        }
    };

    let cancellation = rho_tools::cancellation::RunCancellation::default();
    let (result, timed_out) = if let Some(deadline) = deadline {
        let future = run_session_with_output(
            prompt_text,
            &startup,
            reporter.as_mut(),
            Some(cancellation.clone()),
            jsonl.as_mut(),
        );
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => (result, false),
            () = tokio::time::sleep_until(deadline) => {
                cancellation.cancel();
                (future.await, true)
            }
        }
    } else {
        (
            run_session_with_output(
                prompt_text,
                &startup,
                reporter.as_mut(),
                None,
                jsonl.as_mut(),
            )
            .await,
            false,
        )
    };
    let terminal = classify_run_terminal(result, timed_out);
    if let Some(reporter) = reporter.as_mut() {
        reporter.finish_terminal(&terminal);
    }
    emit_and_exit_terminal(terminal, &mut jsonl, reporter.is_some())
}

fn write_text_answer(answer: &rho_sdk::RunOutcome, has_reporter: bool) -> anyhow::Result<()> {
    let result = (|| -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        if has_reporter {
            writeln!(stdout, "\n[subagent run complete]")?;
        } else {
            writeln!(stdout, "{}", answer.text())?;
        }
        stdout.flush()
    })();
    result.map_err(|error| {
        AutomationExit::new(
            1,
            TerminalReason::OutputError,
            format!("could not write output: {error}"),
        )
        .into()
    })
}

pub(super) fn emit(event: WireEvent) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    write_event(&mut stdout, &event).map_err(|error| {
        AutomationExit::new(
            1,
            TerminalReason::OutputError,
            format!("could not write JSONL output: {error}"),
        )
        .into()
    })
}

fn emit_stopped(adapter: &mut Option<JsonlAdapter>, reason: TerminalReason) -> anyhow::Result<()> {
    if let Some(adapter) = adapter.as_mut() {
        let text = adapter.partial_text();
        let event = adapter.stopped(reason, text);
        emit(event)?;
    }
    Ok(())
}

fn emit_failure(
    adapter: &mut Option<JsonlAdapter>,
    reason: TerminalReason,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    if let Some(adapter) = adapter.as_mut() {
        let text = adapter.partial_text();
        let message = terminal_error_message(reason, error);
        let event = adapter.failed(reason, message, text);
        emit(event)?;
    }
    Ok(())
}

const MAX_STEPS_MESSAGE: &str = "rho run reached its model-step limit";
const TIMEOUT_MESSAGE: &str = "rho run timed out";

/// Single classification of a finished automation session before reporter,
/// JSONL/text emission, and process exit share one decision.
enum RunTerminal {
    Completed(rho_sdk::RunOutcome),
    MaxSteps(rho_sdk::RunOutcome),
    Timeout,
    Failed(anyhow::Error),
}

fn classify_run_terminal(
    result: anyhow::Result<rho_sdk::RunOutcome>,
    timed_out: bool,
) -> RunTerminal {
    if timed_out {
        return RunTerminal::Timeout;
    }
    match result {
        Ok(answer) if answer.stop_reason() == rho_sdk::StopReason::MaxSteps => {
            RunTerminal::MaxSteps(answer)
        }
        Ok(answer) => RunTerminal::Completed(answer),
        Err(error) => RunTerminal::Failed(error),
    }
}

fn emit_and_exit_terminal(
    terminal: RunTerminal,
    jsonl: &mut Option<JsonlAdapter>,
    has_reporter: bool,
) -> anyhow::Result<()> {
    match terminal {
        RunTerminal::Timeout => {
            emit_stopped(jsonl, TerminalReason::Timeout)?;
            Err(AutomationExit::new(124, TerminalReason::Timeout, TIMEOUT_MESSAGE).into())
        }
        RunTerminal::MaxSteps(answer) => {
            if let Some(adapter) = jsonl.as_mut() {
                let text = (!answer.text().is_empty()).then(|| answer.text().into());
                let event = adapter.stopped(TerminalReason::MaxSteps, text);
                emit(event)?;
            } else {
                write_text_answer(&answer, has_reporter)?;
            }
            Err(AutomationExit::new(124, TerminalReason::MaxSteps, MAX_STEPS_MESSAGE).into())
        }
        RunTerminal::Completed(answer) => {
            if let Some(adapter) = jsonl.as_mut() {
                let event = adapter.completed(answer.text().into());
                emit(event)?;
            } else {
                write_text_answer(&answer, has_reporter)?;
            }
            Ok(())
        }
        RunTerminal::Failed(error) => {
            let (reason, code) = classify_error(&error);
            if reason == TerminalReason::Interrupted {
                emit_stopped(jsonl, reason)?;
            } else if reason != TerminalReason::OutputError {
                emit_failure(jsonl, reason, &error)?;
            }
            let message = terminal_error_message(reason, &error);
            if error.is::<AutomationInterrupted>() {
                return Err(error);
            }
            Err(AutomationExit::new(code, reason, message).into())
        }
    }
}

/// Builds the human-readable terminal message for a classified automation error.
///
/// Reason codes stay stable machine labels. The message carries actionable detail
/// except for authentication failures, which stay generic so credentials and
/// token material never leave the process through stdout, stderr, or JSONL.
fn terminal_error_message(reason: TerminalReason, error: &anyhow::Error) -> String {
    match reason {
        TerminalReason::Authentication => "authentication failed".to_string(),
        TerminalReason::ProviderError
        | TerminalReason::ToolHostError
        | TerminalReason::ConfigurationError
        | TerminalReason::OutputError
        | TerminalReason::OtherError
        | TerminalReason::Interrupted
        | TerminalReason::MaxSteps
        | TerminalReason::Timeout
        | TerminalReason::Completed => error.to_string(),
    }
}

fn classify_error(error: &anyhow::Error) -> (TerminalReason, u8) {
    if let Some(interrupted) = error.downcast_ref::<AutomationInterrupted>() {
        return (TerminalReason::Interrupted, interrupted.exit_code());
    }
    if let Some(exit) = error.downcast_ref::<AutomationExit>() {
        return (exit.reason(), exit.exit_code());
    }
    for cause in error.chain() {
        if let Some(error) = cause.downcast_ref::<rho_sdk::Error>() {
            return match error {
                rho_sdk::Error::Authentication { .. } => (TerminalReason::Authentication, 1),
                rho_sdk::Error::Provider(provider)
                    if provider.kind() == rho_sdk::ProviderErrorKind::Authentication =>
                {
                    (TerminalReason::Authentication, 1)
                }
                rho_sdk::Error::Provider(_) => (TerminalReason::ProviderError, 1),
                rho_sdk::Error::Tool(_) => (TerminalReason::ToolHostError, 1),
                rho_sdk::Error::InvalidConfiguration { .. } => {
                    (TerminalReason::ConfigurationError, 2)
                }
                _ => (TerminalReason::OtherError, 1),
            };
        }
        if let Some(error) = cause.downcast_ref::<rho_providers::model::ModelError>() {
            use rho_providers::model::ModelError;
            return match error {
                ModelError::MissingCredentials(_) | ModelError::Credentials(_) => {
                    (TerminalReason::Authentication, 1)
                }
                ModelError::UnsupportedReasoning { .. } | ModelError::UnsupportedProvider(_) => {
                    (TerminalReason::ConfigurationError, 2)
                }
                _ => (TerminalReason::ProviderError, 1),
            };
        }
    }
    (TerminalReason::OtherError, 1)
}

pub(crate) async fn run_session(
    prompt_text: String,
    startup: &Startup<'_>,
    reporter: Option<&mut RunReporter>,
    cancellation: Option<rho_tools::cancellation::RunCancellation>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    ensure_headless_auto_classifier_model(startup.config)?;
    run_session_with_output(prompt_text, startup, reporter, cancellation, None).await
}

async fn run_session_with_output(
    prompt_text: String,
    startup: &Startup<'_>,
    reporter: Option<&mut RunReporter>,
    cancellation: Option<rho_tools::cancellation::RunCancellation>,
    mut jsonl: Option<&mut JsonlAdapter>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    ensure_headless_auto_classifier_model(startup.config)?;
    let SessionAssembly {
        built,
        workspace_root,
    } = assemble_session(SessionAssemblyOptions {
        config: startup.config,
        config_path: startup.config_path.clone(),
        cwd: &startup.cwd,
        no_system_prompt: startup.no_system_prompt,
        no_tools: startup.no_tools,
        no_subagents: startup.no_subagents,
        // Automation keeps questionnaire capability when the agent exposes it.
        questionnaire_enabled: true,
        // An automation run can only show a server's question when the caller
        // supplied a responder for host input; without one the run would fail
        // on the first question instead of declining it.
        mcp_elicitation: match startup.host_input {
            Some(_) => crate::tools::mcp::McpElicitationSupport::Available,
            None => crate::tools::mcp::McpElicitationSupport::Unavailable,
        },
        // Automation binds no model for sampling, so it never declares the
        // capability and rejects any request that arrives anyway.
        mcp_sampling: crate::app::tools_prompt::McpSamplingSupport::Unavailable,
        mcp_attach: crate::app::tools_prompt::McpAttach::Connect,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: &startup.diagnostics,
        agent: &startup.agent,
        max_steps: startup.max_steps,
        usage_purpose: startup.usage_purpose,
        usage_parent_session_id: startup.parent_session_id.clone(),
        hook_host_labels: startup.hook_host_labels.clone(),
        extend_tools: |mut tool_set: crate::tools::sdk_registry::AppToolSet| {
            if let Some(poster) = startup.notice_poster.clone() {
                tool_set.add_bundle(crate::tools::message_parent_bundle(poster));
            }
            tool_set
        },
        approval: |inputs: ApprovalInputs| {
            Ok(SessionApproval {
                session: headless_approval_session(
                    &inputs.config,
                    startup.approval_session.clone(),
                    startup.approval_classifier.clone(),
                    inputs.workspace_root,
                    inputs.usage_recording,
                    inputs.session_writes,
                )?,
                receiver: None,
            })
        },
        session_options: |_| Ok(crate::app::interactive_runtime::startup::fresh_session_options()),
    })
    .await?;
    let session = &built.session;
    if let Some(adapter) = jsonl.as_deref_mut() {
        adapter.set_run_context(session.id(), &workspace_root);
    }
    startup
        .herdr
        .report_state(HerdrState::Working, None, None)
        .await;
    let result = complete_run(
        session,
        prompt_text,
        HeadlessRunDeps {
            reporter,
            external_cancellation: cancellation,
            jsonl,
            host_input: startup.host_input.as_deref(),
        },
        startup.steering_slot.clone(),
    )
    .await;

    let session_hooks = built.runtime.hooks();
    let session_id = session.id().clone();
    match &result {
        Ok(_) => {
            session_hooks.session_completed(&session_id, /* completed_runs */ 1)
        }
        Err(error) => session_hooks.session_failed(
            &session_id,
            rho_sdk::hooks::HookSessionFailureKind::RunFailed,
            &error.to_string(),
        ),
    }
    built.teardown().await;
    startup
        .herdr
        .report_state(HerdrState::Idle, None, None)
        .await;
    startup.herdr.release().await;

    result
}

pub(crate) fn ensure_headless_auto_classifier_model(config: &Config) -> anyhow::Result<()> {
    if config.permission_mode == PermissionMode::Auto
        && config
            .internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)
            .is_none()
    {
        anyhow::bail!(
            "permission mode auto requires a configured permission-classifier model (set via /config or config.toml [internal_agents.permission-classifier])"
        );
    }
    Ok(())
}

/// Resolves the approval session for one headless run.
///
/// Non-Auto keeps the inherited session. Auto always installs a classifier:
/// isolate a workflow/subagent template onto this run's write log when present,
/// otherwise build a fresh headless classifier. A stray non-classifier
/// `approval_session` is ignored in Auto so callers do not juggle paired Option
/// knobs.
fn headless_approval_session(
    config: &Config,
    approval_session: Option<rho_sdk::ApprovalSession>,
    approval_classifier: Option<Arc<ClassifierApprovalHandler>>,
    workspace_root: PathBuf,
    usage_recording: rho_sdk::ProviderRequestUsageRecording,
    session_writes: SessionWriteLog,
) -> anyhow::Result<Option<rho_sdk::ApprovalSession>> {
    if config.permission_mode != PermissionMode::Auto {
        return Ok(approval_session);
    }
    Ok(Some(rho_sdk::ApprovalSession::from_shared(
        headless_auto_classifier(
            config,
            approval_classifier,
            workspace_root,
            usage_recording,
            session_writes,
        ),
    )))
}

fn headless_auto_classifier(
    config: &Config,
    approval_classifier: Option<Arc<ClassifierApprovalHandler>>,
    workspace_root: PathBuf,
    usage_recording: rho_sdk::ProviderRequestUsageRecording,
    session_writes: SessionWriteLog,
) -> Arc<ClassifierApprovalHandler> {
    match approval_classifier {
        Some(template) => template.isolate_for_run(session_writes),
        None => ClassifierApprovalHandler::shared(
            config.clone(),
            workspace_root,
            usage_recording,
            None,
            Some(session_writes),
        ),
    }
}

async fn complete_run(
    session: &rho_sdk::Session,
    prompt_text: String,
    dependencies: HeadlessRunDeps<'_>,
    steering_slot: Option<super::subagent_messaging::SteeringSlot>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    let HeadlessRunDeps {
        reporter,
        external_cancellation,
        jsonl,
        host_input,
    } = dependencies;
    let mut run = session.start(UserInput::text(prompt_text)).await?;
    if let Some(slot) = steering_slot {
        slot.publish(run.steering_handle());
    }
    let cancellation = run.cancellation_handle();
    let external_cancellation = external_cancellation.unwrap_or_default();
    tokio::select! {
        outcome = headless_run::drive(&mut run, reporter, jsonl, host_input) => outcome,
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

pub(crate) use crate::run_artifacts::RunArtifactIdentity;

/// Maintains the `--output-file` status contract for subagent runs and
/// streams progress to stdout so a watching pane shows live activity.
pub(crate) struct RunReporter {
    sink: crate::run_artifacts::RunArtifactSink,
    adapter: crate::tui::event_adapter::SdkEventAdapter,
    stream_output: bool,
}

impl RunReporter {
    pub(crate) fn new(
        path: PathBuf,
        identity: RunArtifactIdentity,
        cwd: PathBuf,
        prompt: &str,
        stream_output: bool,
        status_tx: Option<tokio::sync::watch::Sender<RunStatus>>,
    ) -> anyhow::Result<Self> {
        let sink = crate::run_artifacts::RunArtifactSink::open(path, &identity, prompt, status_tx)?;
        Ok(Self {
            sink,
            adapter: crate::tui::event_adapter::SdkEventAdapter::new(cwd),
            stream_output,
        })
    }

    /// Resume after the executor already wrote the Starting boundary.
    pub(crate) fn continue_from(
        path: PathBuf,
        started_status: RunStatus,
        cwd: PathBuf,
        prompt: &str,
        stream_output: bool,
        status_tx: Option<tokio::sync::watch::Sender<RunStatus>>,
        live_title: Option<crate::run_artifacts::LiveRunTitle>,
    ) -> anyhow::Result<Self> {
        let sink = crate::run_artifacts::RunArtifactSink::continue_from(
            path,
            started_status,
            prompt,
            status_tx,
            live_title,
        )?;
        Ok(Self {
            sink,
            adapter: crate::tui::event_adapter::SdkEventAdapter::new(cwd),
            stream_output,
        })
    }

    pub(super) fn on_event(&mut self, event: &rho_sdk::RunEvent) {
        use rho_sdk::RunEvent;

        let attachments = crate::tui::translate_run_event(&mut self.adapter, event);
        for attachment in attachments {
            // Reasoning is deliberately kept out of `last_text`: the status file
            // carries the answer, not the thinking.
            if let crate::run_artifacts::AttachmentEvent::AssistantTextDelta(text) = &attachment {
                if !text.is_empty() {
                    self.sink.append_last_text(text);
                }
            }
            self.sink.record_attachment(attachment);
        }
        match event {
            RunEvent::StepStarted { step, .. } => {
                self.sink.status.state = RunState::Running;
                self.sink.status.turns = *step as u64;
                self.sink.publish();
            }
            RunEvent::ToolStarted { name, .. } => {
                self.sink.status.last_activity = Some(format!("tool: {name}"));
                self.stream(&format!("\n[tool] {name}\n"));
                self.sink.publish();
            }
            RunEvent::HostInputRequested { request }
            | RunEvent::ToolHostInputRequested { request, .. } => {
                self.sink.status.last_activity =
                    Some(format!("waiting for questionnaire: {}", request.title()));
                self.sink.publish();
            }
            RunEvent::AssistantTextDelta { text } => {
                self.sink.status.last_activity = Some("assistant text".into());
                self.stream(text);
                // Attachment path already published throttled when translated.
            }
            RunEvent::ProviderStreamReset { .. } => {
                self.sink.status.last_activity = Some("retrying provider response".into());
                self.sink.status.last_text = None;
                self.stream("\n[provider response discarded; retrying]\n");
                self.sink.publish();
            }
            RunEvent::UsageUpdated { usage } => {
                self.sink.status.input_tokens = usage.inclusive_prompt_tokens();
                self.sink.status.output_tokens = usage.output_tokens;
            }
            _ => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> &RunStatus {
        &self.sink.status
    }

    pub(super) fn write(&mut self) {
        self.sink.publish();
    }

    pub(crate) fn finish(&mut self, result: &anyhow::Result<rho_sdk::RunOutcome>) {
        match result {
            Ok(outcome) => {
                let usage = outcome.usage();
                self.sink.status.input_tokens = usage.inclusive_prompt_tokens();
                self.sink.status.output_tokens = usage.output_tokens;
                self.sink.finish_ok(Some(outcome.text().to_string()));
            }
            Err(error)
                if error.is::<AutomationInterrupted>()
                    || error.downcast_ref::<AutomationExit>().is_some_and(|exit| {
                        matches!(
                            exit.reason(),
                            TerminalReason::MaxSteps | TerminalReason::Timeout
                        )
                    })
                    || error.is::<SubagentCancelled>() =>
            {
                self.sink.finish_stopped("stopped");
            }
            Err(error) => {
                self.sink.finish_error(format!("{error:#}"));
            }
        }
    }

    fn finish_terminal(&mut self, terminal: &RunTerminal) {
        match terminal {
            RunTerminal::Completed(outcome) => {
                let usage = outcome.usage();
                self.sink.status.input_tokens = usage.inclusive_prompt_tokens();
                self.sink.status.output_tokens = usage.output_tokens;
                self.sink.finish_ok(Some(outcome.text().to_string()));
            }
            RunTerminal::MaxSteps(_) | RunTerminal::Timeout => {
                self.sink.finish_stopped("stopped");
            }
            RunTerminal::Failed(error)
                if error.is::<AutomationInterrupted>() || error.is::<SubagentCancelled>() =>
            {
                self.sink.finish_stopped("stopped");
            }
            RunTerminal::Failed(error) => {
                self.sink.finish_error(format!("{error:#}"));
            }
        }
    }

    fn stream(&self, text: &str) {
        if !self.stream_output {
            return;
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
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
    if !read_stdin && crate::stdio::stdin_is_redirected() {
        anyhow::bail!(
            "stdin is redirected but --stdin was not set; pass --stdin to include piped input"
        );
    }
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
