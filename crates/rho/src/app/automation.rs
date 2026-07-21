use std::{
    fmt,
    io::{self, Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use rho_sdk::{SessionOptions, SystemPrompt, UserInput, Workspace};

use {
    crate::agent::{PromptPolicy, ToolCapability},
    crate::cli::{Command, OutputFormat},
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::{HerdrReporter, HerdrState},
    crate::prompt,
    crate::subagent::{self, RunState, RunStatus},
    crate::tools::{
        agent::BackgroundSubagents,
        sdk_registry::{AppToolSet, DelegationConfig, ToolSetOptions},
    },
    crate::tui::AttachmentWriter,
    rho_providers::credentials::OsCredentialStore,
    rho_providers::providers::build_automation_provider,
};

use super::{
    agent_binding::BoundAgent,
    automation_protocol::{write_event, JsonlAdapter, TerminalReason, WireEvent},
    policy::AppPolicy,
    runtime_builder::{
        build_runtime_with_max_steps, configured_context_window, RuntimeBuildOptions,
    },
    sdk_config::SdkBootstrapOptions,
};

/// Error returned after an automation run has cleaned up and selected a stable exit code.
#[derive(Debug)]
pub struct AutomationExit {
    code: u8,
    reason: TerminalReason,
    message: String,
}

impl AutomationExit {
    /// Creates an automation exit with a process code, terminal reason, and message.
    ///
    /// # Examples
    ///
    /// ```
    /// let exit = AutomationExit::new(1, TerminalReason::OtherError, "run failed");
    /// assert_eq!(exit.exit_code(), 1);
    /// ```
    ///
    /// # Parameters
    ///
    /// * `code` - The process exit code.
    /// * `reason` - The terminal reason associated with the exit.
    /// * `message` - The message describing the exit.
    pub(super) fn new(code: u8, reason: TerminalReason, message: impl Into<String>) -> Self {
        Self {
            code,
            reason,
            message: message.into(),
        }
    }

    /// Provides the process exit code associated with this automation result.
    ///
    /// # Examples
    ///
    /// ```
    /// let result = AutomationExit::new(124, TerminalReason::Timeout, "timed out".into());
    /// assert_eq!(result.exit_code(), 124);
    /// ```
    pub fn exit_code(&self) -> u8 {
        self.code
    }

    /// Gets the terminal reason associated with the automation exit.
    ///
    /// # Examples
    ///
    /// ```
    /// let exit = AutomationExit::new(1, TerminalReason::OtherError, "failed");
    /// assert_eq!(exit.reason(), TerminalReason::OtherError);
    /// ```
    fn reason(&self) -> TerminalReason {
        self.reason
    }
}

impl fmt::Display for AutomationExit {
    /// Formats the error using its message.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = AutomationExit::new(1, TerminalReason::OtherError, "run failed".to_owned());
    /// assert_eq!(format!("{error}"), "run failed");
    /// ```
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
    /// Maps the shutdown signal to its process exit code.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ShutdownSignal::Interrupt.exit_code(), 130);
    /// assert_eq!(ShutdownSignal::Terminate.exit_code(), 143);
    /// ```
    fn exit_code(self) -> u8 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }
}

impl fmt::Display for ShutdownSignal {
    /// Formats the shutdown signal using its conventional name.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ShutdownSignal::Interrupt.to_string(), "SIGINT");
    /// assert_eq!(ShutdownSignal::Terminate.to_string(), "SIGTERM");
    /// ```
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
}

/// Builds the prompt for a run command, including input read from standard input when requested.
///
/// Commands that do not start an automation run produce no prompt.
///
/// # Examples
///
/// ```
/// let command = Some(Command::Run {
///     prompt: vec!["List the files".to_owned()],
///     stdin: false,
///     ..
/// });
///
/// assert_eq!(
///     prompt_for_command(&command).unwrap(),
///     Some("List the files".to_owned())
/// );
/// ```
pub(super) fn prompt_for_command(command: &Option<Command>) -> anyhow::Result<Option<String>> {
    match command {
        Some(Command::Run { prompt, stdin, .. }) => {
            prompt_from_stdin(prompt.clone(), *stdin).map(Some)
        }
        Some(Command::Attach { .. } | Command::Login { .. } | Command::Update) | None => Ok(None),
    }
}

/// Emits a JSONL event indicating that startup failed due to configuration.
///
/// # Examples
///
/// ```
/// assert!(emit_startup_failure().is_ok());
/// ```
pub(super) fn emit_startup_failure() -> anyhow::Result<()> {
    let mut adapter = JsonlAdapter::new();
    let event = adapter.failed(
        TerminalReason::ConfigurationError,
        "configuration failed".into(),
        None,
    );
    emit(event)
}

/// Runs an automation session and emits its result in the configured output format.
///
/// Applies the configured timeout and step limit, updates any configured run
/// report, and maps terminal conditions to stable automation exit errors.
///
/// # Examples
///
/// ```ignore
/// let result = run(prompt, startup).await;
/// ```
pub(super) async fn run(prompt_text: String, startup: Startup<'_>) -> anyhow::Result<()> {
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
                RunArtifactIdentity {
                    agent_id: startup.agent.id().to_string(),
                    agent_fingerprint: startup.agent.fingerprint().to_string(),
                    provider: startup.config.provider.clone(),
                    model: startup.config.model.clone(),
                },
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
    if let Some(reporter) = reporter.as_mut() {
        let reached_step_limit = result.as_ref().is_ok_and(|outcome| {
            outcome.stop_reason() == rho_sdk::StopReason::MaxSteps
                && (jsonl.is_some() || startup.max_steps.is_some())
        });
        if reached_step_limit {
            let stopped = Err(AutomationExit::new(
                124,
                TerminalReason::MaxSteps,
                "rho run reached its model-step limit",
            )
            .into());
            reporter.finish(&stopped);
        } else {
            reporter.finish(&result);
        }
    }

    if timed_out {
        emit_stopped(&mut jsonl, TerminalReason::Timeout)?;
        return Err(AutomationExit::new(124, TerminalReason::Timeout, "rho run timed out").into());
    }

    match result {
        Ok(answer) => {
            let max_steps = answer.stop_reason() == rho_sdk::StopReason::MaxSteps;
            if max_steps && (jsonl.is_some() || startup.max_steps.is_some()) {
                if let Some(adapter) = jsonl.as_mut() {
                    let text = (!answer.text().is_empty()).then(|| answer.text().into());
                    let event = adapter.stopped(TerminalReason::MaxSteps, text);
                    emit(event)?;
                } else {
                    write_text_answer(&answer, reporter.is_some())?;
                }
                return Err(AutomationExit::new(
                    124,
                    TerminalReason::MaxSteps,
                    "rho run reached its model-step limit",
                )
                .into());
            }
            if let Some(adapter) = jsonl.as_mut() {
                let event = adapter.completed(answer.text().into());
                emit(event)?;
            } else {
                write_text_answer(&answer, reporter.is_some())?;
            }
            Ok(())
        }
        Err(error) => {
            let (reason, code) = classify_error(&error);
            if reason == TerminalReason::Interrupted {
                emit_stopped(&mut jsonl, reason)?;
            } else if reason != TerminalReason::OutputError {
                emit_failure(&mut jsonl, reason, &error)?;
            }
            let message = terminal_error_message(reason, &error);
            if error.is::<AutomationInterrupted>() {
                return Err(error);
            }
            Err(AutomationExit::new(code, reason, message).into())
        }
    }
}

/// Writes the run's answer to standard output, or signals completion when a reporter is active.
///
/// # Parameters
///
/// * `answer` — The completed run outcome whose text is written when no reporter is active.
/// * `has_reporter` — Whether to emit the subagent completion marker instead of the answer text.
///
/// # Returns
///
/// `Ok(())` after the output is written; an `AutomationExit` with an output-error reason if writing or flushing fails.
///
/// # Examples
///
/// ```no_run
/// write_text_answer(&answer, false)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
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

/// Writes a JSONL event to standard output.
///
/// # Examples
///
/// ```ignore
/// emit(event)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// Returns an output error if the event cannot be written.
fn emit(event: WireEvent) -> anyhow::Result<()> {
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

/// Emits a stopped event containing the adapter's current partial text when JSONL output is enabled.
///
/// # Examples
///
/// ```
/// let mut adapter: Option<JsonlAdapter> = None;
/// emit_stopped(&mut adapter, TerminalReason::Timeout)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
fn emit_stopped(adapter: &mut Option<JsonlAdapter>, reason: TerminalReason) -> anyhow::Result<()> {
    if let Some(adapter) = adapter.as_mut() {
        let text = adapter.partial_text();
        let event = adapter.stopped(reason, text);
        emit(event)?;
    }
    Ok(())
}

/// Emits a failed JSONL event with the current partial text when an adapter is available.
///
/// # Examples
///
/// ```
/// let mut adapter = None;
/// let error = anyhow::anyhow!("run failed");
///
/// emit_failure(&mut adapter, TerminalReason::OtherError, &error)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
?
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

/// Creates a user-facing message for a terminal run reason.

///

/// # Examples

///

/// ```

/// let error = anyhow::anyhow!("provider unavailable");

/// assert_eq!(

///     terminal_error_message(TerminalReason::Authentication, &error),

///     "authentication failed"

/// );

/// assert_eq!(

///     terminal_error_message(TerminalReason::ProviderError, &error),

///     "provider unavailable"

/// );

/// ```

///

/// `reason` determines whether a standardized message is available; otherwise,

/// the original error message is returned.
fn terminal_error_message(reason: TerminalReason, error: &anyhow::Error) -> String {
    match reason {
        TerminalReason::Authentication => "authentication failed".to_string(),
        TerminalReason::ConfigurationError => "configuration failed".to_string(),
        TerminalReason::OutputError => "output failed".to_string(),
        TerminalReason::OtherError => "run failed".to_string(),
        _ => error.to_string(),
    }
}

/// Classifies an automation error into a terminal reason and process exit code.
///
/// # Examples
///
/// ```
/// let error = anyhow::anyhow!("unexpected failure");
/// assert_eq!(classify_error(&error), (TerminalReason::OtherError, 1));
/// ```
///
/// Authentication and provider errors are mapped to their corresponding terminal
/// reasons, while configuration errors use exit code `2`.
fn classify_error(error: &anyhow::Error) -> (TerminalReason, u8)
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
                ModelError::MissingApiKey
                | ModelError::MissingCodexAuth
                | ModelError::MissingAnthropicApiKey
                | ModelError::MissingGoogleApiKey
                | ModelError::MissingGithubCopilotAuth
                | ModelError::MissingMoonshotApiKey
                | ModelError::MissingOpenRouterApiKey
                | ModelError::MissingKimiAuth
                | ModelError::MissingXaiApiKey
                | ModelError::MissingXaiAuth
                | ModelError::Credentials(_) => (TerminalReason::Authentication, 1),
                ModelError::UnsupportedReasoning { .. } | ModelError::UnsupportedProvider(_) => {
                    (TerminalReason::ConfigurationError, 2)
                }
                _ => (TerminalReason::ProviderError, 1),
            };
        }
    }
    (TerminalReason::OtherError, 1)
}

/// Runs an automation session without JSONL event output.
///
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let outcome = run_session("List the files in the workspace".into(), &todo!(), None, None).await?;
/// println!("{}", outcome.text());
/// # Ok(())
/// # }
/// ```
///
/// # Returns
///
/// The completed automation outcome.
pub(crate) async fn run_session(
    prompt_text: String,
    startup: &Startup<'_>,
    reporter: Option<&mut RunReporter>,
    cancellation: Option<rho_tools::cancellation::RunCancellation>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    run_session_with_output(prompt_text, startup, reporter, cancellation, None).await
}

/// Builds the configured runtime and executes an automation session.
///
/// Applies startup settings for providers, tools, prompts, workspace access, delegation, and step limits.
/// Optional reporting, cancellation, and JSONL event streaming are integrated into the session lifecycle.
///
/// # Parameters
///
/// * `prompt_text` - The user prompt to execute.
/// * `startup` - Runtime configuration and agent startup resources.
/// * `reporter` - Optional reporter for recording session progress and results.
/// * `cancellation` - Optional external cancellation handle.
/// * `jsonl` - Optional adapter for emitting session events.
///
/// # Returns
///
/// The completed automation run outcome.
///
/// # Examples
///
/// ```ignore
/// let outcome = run_session_with_output(
///     "List the files in the workspace".into(),
///     &startup,
///     None,
///     None,
///     None,
/// ).await?;
/// println!("{}", outcome.text());
/// # Ok::<(), anyhow::Error>(())
/// ```
async fn run_session_with_output(
    prompt_text: String,
    startup: &Startup<'_>,
    reporter: Option<&mut RunReporter>,
    cancellation: Option<rho_tools::cancellation::RunCancellation>,
    mut jsonl: Option<&mut JsonlAdapter>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    let sdk_options = SdkBootstrapOptions::from_config(startup.config, &startup.cwd)?;
    let credentials = rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(OsCredentialStore),
    );
    let provider = build_automation_provider(sdk_options.provider, &credentials)?;
    let mut capabilities = startup.agent.capabilities().clone();
    if startup.no_subagents {
        capabilities.remove(&ToolCapability::Agent);
        capabilities.remove(&ToolCapability::Agents);
    }
    let launch_delegation_enabled = capabilities.contains(&ToolCapability::Agent);
    let delegation_enabled =
        launch_delegation_enabled || capabilities.contains(&ToolCapability::Agents);
    let tool_set = if startup.no_tools {
        AppToolSet::disabled()
    } else {
        let mut options = ToolSetOptions::new(capabilities);
        if delegation_enabled {
            options = options.delegation(DelegationConfig::new(
                startup.cwd.clone(),
                startup.config_path.clone(),
                BackgroundSubagents::Disabled,
            ));
        }
        AppToolSet::new(startup.config, startup.diagnostics.clone(), options)
    };
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

    let workspace_root = sdk_options.workspace.root.clone();
    let workspace = Workspace::new(&workspace_root)?;
    let context_window = configured_context_window(startup.config);
    let compaction = sdk_options.runtime.compaction.clone();
    startup.diagnostics.update_compaction_config(&compaction);
    let usage_recording = crate::usage::default_recording().await;
    let runtime = build_runtime_with_max_steps(
        RuntimeBuildOptions {
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
        },
        startup.max_steps,
    )?;
    let session = runtime.session(SessionOptions::default()).await?;
    if let Some(adapter) = jsonl.as_deref_mut() {
        adapter.set_run_context(session.id(), &workspace_root);
    }
    if let Some(manager) = tool_set.subagents() {
        manager.set_session(session.id().to_string());
    }

    startup
        .herdr
        .report_state(HerdrState::Working, None, None)
        .await;
    let result = complete_run(&session, prompt_text, reporter, cancellation, jsonl).await;

    runtime.shutdown();
    tool_set.shutdown().await;
    startup
        .herdr
        .report_state(HerdrState::Idle, None, None)
        .await;
    startup.herdr.release().await;

    result
}

/// Runs a session until completion, cancellation, or an operating-system shutdown signal.
///
/// The run is cancelled and drained before returning when external cancellation or a shutdown
/// signal is received.
///
/// # Examples
///
/// ```ignore
/// let outcome = complete_run(session, prompt, None, None, None).await?;
/// println!("{}", outcome.text());
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Errors
///
/// Returns an error if the session cannot start or if execution is interrupted, cancelled, or
/// otherwise fails.
async fn complete_run(
session: &rho_sdk::Session,
prompt_text: String,
reporter: Option<&mut RunReporter>,
external_cancellation: Option<rho_tools::cancellation::RunCancellation>,
jsonl: Option<&mut JsonlAdapter>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
async fn complete_run(
    session: &rho_sdk::Session,
    prompt_text: String,
    reporter: Option<&mut RunReporter>,
    external_cancellation: Option<rho_tools::cancellation::RunCancellation>,
    jsonl: Option<&mut JsonlAdapter>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    let mut run = session.start(UserInput::text(prompt_text)).await?;
    let cancellation = run.cancellation_handle();
    let external_cancellation = external_cancellation.unwrap_or_default();
    tokio::select! {
        outcome = drive_headless_run(&mut run, reporter, jsonl) => outcome,
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

/// Processes a run without interactive host input, updating reporting outputs as events arrive.
///
/// Host input requests cancel the run and return an error because they cannot be answered
/// in headless mode. Reporter heartbeats are written periodically, and JSONL events are
/// emitted when configured.
///
/// # Examples
///
/// ```no_run
/// # async fn example(run: &mut rho_sdk::Run) -> anyhow::Result<()> {
/// let outcome = drive_headless_run(run, None, None).await?;
/// # let _ = outcome;
/// # Ok(())
/// # }
/// ```
async fn drive_headless_run(
    run: &mut rho_sdk::Run,
    mut reporter: Option<&mut RunReporter>,
    mut jsonl: Option<&mut JsonlAdapter>,
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
        if let Some(adapter) = jsonl.as_deref_mut() {
            if let Some(wire_event) = adapter.event(&event) {
                if let Err(error) = emit(wire_event) {
                    run.cancel();
                    let _ = run.outcome().await;
                    return Err(error);
                }
            }
        }
        let request = match event {
            rho_sdk::RunEvent::HostInputRequested { request }
            | rho_sdk::RunEvent::ToolHostInputRequested { request, .. } => Some(request),
            _ => None,
        };
        if let Some(request) = request {
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
            RunEvent::ProviderStreamReset { .. } => {
                self.status.last_activity = Some("retrying provider response".into());
                self.status.last_text = None;
                self.stream("\n[provider response discarded; retrying]\n");
                self.write();
            }
            RunEvent::UsageUpdated { usage } => {
                self.status.input_tokens = usage.total_input_tokens().unwrap_or(0);
                self.status.output_tokens = usage.output_tokens.unwrap_or(0);
            }
            _ => {}
        }
    }

    /// Finalizes the run report from the completed outcome.
    ///
    /// Successful runs are marked as completed with their result and token usage. Interruptions,
    /// cancellations, timeouts, and step-limit stops are marked as stopped; other failures are
    /// marked as errors. The updated status is written after processing the result.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn example(reporter: &mut RunReporter, result: &anyhow::Result<rho_sdk::RunOutcome>) {
    /// reporter.finish(result);
    /// # }
    /// ```
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
                if error.is::<AutomationInterrupted>()
                    || error.downcast_ref::<AutomationExit>().is_some_and(|exit| {
                        matches!(
                            exit.reason(),
                            TerminalReason::MaxSteps | TerminalReason::Timeout
                        )
                    })
                    || error.is::<SubagentCancelled>() =>
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
