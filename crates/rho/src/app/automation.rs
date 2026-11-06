//! `rho run`: one prompt on a saved session with no person watching, its
//! events written as text or as the versioned JSONL stream (see
//! `automation_protocol.rs`), ending with a documented exit code.
//!
//! The session runs on the headless host (`headless_host.rs`); this module
//! owns what is `rho run`'s own: reading the prompt, resolving its slash
//! command, the terminal event, and the exit code.

use std::{
    fmt,
    io::{self, Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
    time::Duration,
};

use rho_sdk::{CancellationToken, UserInput};

use super::{
    agent_binding::BoundAgent,
    automation_protocol::{write_event, JsonlAdapter, TerminalReason, WireEvent},
    headless_host::{
        cancel_on_stop, until_stopped, HeadlessView, PromptEnd, RunReporter, StopCause, Stops,
    },
    session_commands::{self, BuiltInCommand, CommandScope, CommandSet, PromptCommand},
    session_runtime::{
        CommandRejected, HostCapabilities, Logouts, RunShape, SessionBuild, SessionLaunch,
        SessionOpening, SessionRole, SessionRuntime, Turn, TurnOutcome, UnattendedApprovals,
    },
    shutdown_signal::{ShutdownSignal, ShutdownSignals},
};
use crate::{
    cli::{Command, OutputFormat},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    herdr::{HerdrReporter, HerdrState},
};

mod terminal;
use terminal::{classify_error, TerminalMessages};

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

/// What `rho run` was launched with.
pub(super) struct Startup<'a> {
    pub config: &'a Config,
    pub config_path: PathBuf,
    pub cwd: PathBuf,
    pub no_system_prompt: bool,
    pub no_tools: bool,
    pub no_subagents: bool,
    pub agent: BoundAgent,
    pub output_file: Option<PathBuf>,
    pub output: OutputFormat,
    pub max_steps: Option<NonZeroUsize>,
    pub timeout: Option<Duration>,
    pub diagnostics: RuntimeDiagnostics,
    pub herdr: HerdrReporter,
    /// Directories added to the workspace scope beyond `cwd`.
    pub added_dirs: crate::added_dirs::AddedDirs,
}

/// The commands `rho run` runs. `/compact` is not among them: a run's
/// session is new, so there is nothing to compact.
const RUN_COMMANDS: CommandSet = CommandSet {
    built_ins: &[BuiltInCommand::Init],
    sessions_label: "rho run",
};

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
            | Command::Computer { .. }
            | Command::ModelPrompt { .. }
            | Command::Plugins { .. }
            | Command::Workflow { .. }
            | Command::WorkflowPlannerWorker
            | Command::CompactionEval(_)
            | Command::ClassifierEval(_)
            | Command::Update
            | Command::Uninstall { .. }
            | Command::Doctor { .. }
            | Command::Acp(_)
            | Command::Daemon { .. },
        )
        | None => Ok(None),
        #[cfg(debug_assertions)]
        Some(Command::AcpFixtureAgent(_)) => Ok(None),
    }
}

pub(super) fn emit_startup_failure(message: impl Into<String>) -> anyhow::Result<()> {
    let mut adapter = JsonlAdapter::new();
    let event = adapter.failed(TerminalReason::ConfigurationError, message.into(), None);
    emit(event)
}

pub(super) async fn run(prompt_text: String, startup: Startup<'_>) -> anyhow::Result<()> {
    let mut jsonl = (startup.output == OutputFormat::Jsonl).then(JsonlAdapter::new);
    // The bound agent may pin its own provider and auth; that is what runs.
    let selection = startup.agent.rho_config().unwrap_or(startup.config);
    let messages = TerminalMessages::for_selection(&selection.provider, &selection.auth);
    let deadline = startup
        .timeout
        .map(|timeout| tokio::time::Instant::now() + timeout);
    // Before any run state is published, so a signal during startup is held
    // for the run to handle instead of killing the process mid-setup.
    let mut stops = Stops::on_signals(ShutdownSignals::install()?).or_at(deadline);
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
            emit_failure(&mut jsonl, TerminalReason::OutputError, &error, &messages)?;
            return Err(
                AutomationExit::new(1, TerminalReason::OutputError, error.to_string()).into(),
            );
        }
    };

    let end = run_session(
        prompt_text,
        &startup,
        &mut stops,
        reporter.as_mut(),
        jsonl.as_mut(),
    )
    .await;
    let terminal = classify_run_terminal(end);
    if let Some(reporter) = reporter.as_mut() {
        match &terminal {
            RunTerminal::Completed(outcome) => reporter.finish_ok(outcome),
            RunTerminal::MaxSteps(_) | RunTerminal::Timeout | RunTerminal::Interrupted(_) => {
                reporter.finish_stopped();
            }
            RunTerminal::Failed(error) => reporter.finish_error(error),
        }
    }
    emit_and_exit_terminal(terminal, &mut jsonl, reporter.is_some(), &messages)
}

/// Runs the prompt on a new session saved as `rho run`'s, until it ends or
/// `stops` stops it, and tears the session down. Returns how the prompt
/// ended. The session is never titled: a model call per scripted run is not
/// worth it, and listings fall back to its first message.
async fn run_session(
    prompt_text: String,
    startup: &Startup<'_>,
    stops: &mut Stops,
    reporter: Option<&mut RunReporter>,
    jsonl: Option<&mut JsonlAdapter>,
) -> PromptEnd {
    let build = SessionBuild {
        launch: SessionLaunch {
            config_path: startup.config_path.clone(),
            no_system_prompt: startup.no_system_prompt,
            no_tools: startup.no_tools,
            no_subagents: startup.no_subagents,
            diagnostics: startup.diagnostics.clone(),
            agent: startup.agent.clone(),
            prompt_suffix: None,
        },
        host: HostCapabilities::unattended(UnattendedApprovals::Alone),
        run: RunShape {
            role: SessionRole::Run,
            max_steps: startup.max_steps,
            hook_labels: Default::default(),
        },
    };
    let logouts = Logouts::default();
    let opening = SessionOpening {
        build,
        process_config: startup.config,
        host_mcp: Default::default(),
        cwd: &startup.cwd,
        added_dirs: startup.added_dirs.clone(),
        logouts: &logouts,
    };
    let mut view = HeadlessView::new(reporter, jsonl);
    let stop = CancellationToken::new();
    let work = async {
        let mut runtime = SessionRuntime::open_new(opening, rho_sdk::SessionId::new()).await?;
        view.bind(&runtime);
        let _ = startup
            .herdr
            .report_state(HerdrState::Working, None, None)
            .await;
        let ended = cancel_on_stop(
            &stop,
            runtime.handle(),
            run_prompt(&mut runtime, &mut view, prompt_text, startup.config),
        )
        .await;
        runtime.shutdown().await;
        let _ = startup
            .herdr
            .report_state(HerdrState::Idle, None, None)
            .await;
        let _ = startup.herdr.release().await;
        anyhow::Ok(ended)
    };
    let (worked, cause) = until_stopped(stops, &stop, work).await;
    // A JSONL write that failed already failed the turn; the stream is gone.
    if let Some(message) = view.take_output_failure() {
        let failed = AutomationExit::new(1, TerminalReason::OutputError, message);
        return PromptEnd::Failed(failed.into());
    }
    match worked {
        Ok(ended) => PromptEnd::of(ended, cause),
        Err(error) => PromptEnd::of(Err(error), cause),
    }
}

/// Runs `text` within a new prompt: its slash command when it starts with
/// one `rho run` runs, else a model turn on it. A command `rho run` cannot
/// run fails the prompt as a configuration error.
async fn run_prompt(
    runtime: &mut SessionRuntime,
    view: &mut HeadlessView<'_>,
    text: String,
    process_config: &Config,
) -> anyhow::Result<TurnOutcome> {
    let prompt = runtime.begin_prompt(process_config).await?;
    let input = UserInput::text(text);
    let home = crate::paths::home_dir();
    let command = {
        let scope = CommandScope::of(runtime, home.as_deref(), process_config);
        // Expansion can wait on an MCP server, so a stop drops it.
        tokio::select! {
            biased;
            () = prompt.cancellation().cancelled() => return Ok(TurnOutcome::Cancelled),
            command = session_commands::resolve(input.clone(), &scope, RUN_COMMANDS) => command,
        }
    };
    let rejected = |message: String| -> anyhow::Error {
        AutomationExit::new(2, TerminalReason::ConfigurationError, message).into()
    };
    match command.map_err(|rejected_command| rejected(rejected_command.to_string()))? {
        PromptCommand::Turn {
            input: model,
            calls,
        } => {
            let turn = Turn {
                display: input,
                model,
                calls,
            };
            runtime
                .run_prompt_turn(view, &prompt, turn, process_config)
                .await
        }
        PromptCommand::RunAsAgent(task) => {
            runtime
                .rebind_agent(&prompt, &task, process_config)
                .await
                .map_err(|error| match error.downcast::<CommandRejected>() {
                    Ok(CommandRejected(message)) => rejected(message),
                    Err(error) => error,
                })?;
            // The command stays the shown message; the model gets the task.
            let turn = Turn {
                display: input,
                model: task.input,
                calls: Vec::new(),
            };
            runtime
                .run_prompt_turn(view, &prompt, turn, process_config)
                .await
        }
        // `RUN_COMMANDS` has no `/compact`, so the resolver rejects it first.
        PromptCommand::Compact { .. } => Err(rejected(
            "could not run /compact: not available in rho run".into(),
        )),
        // The workflow would outlive the process, which exits with its prompt.
        PromptCommand::StartWorkflow(start) => Err(rejected(format!(
            "could not run /workflow:{}: not available in rho run; use rho workflow run",
            start.workflow.label
        ))),
    }
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
    messages: &TerminalMessages,
) -> anyhow::Result<()> {
    if let Some(adapter) = adapter.as_mut() {
        let text = adapter.partial_text();
        let message = messages.for_error(reason, error);
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
    Interrupted(ShutdownSignal),
    Failed(anyhow::Error),
}

fn classify_run_terminal(end: PromptEnd) -> RunTerminal {
    match end {
        PromptEnd::Finished(answer) if answer.stop_reason() == rho_sdk::StopReason::MaxSteps => {
            RunTerminal::MaxSteps(answer)
        }
        PromptEnd::Finished(answer) => RunTerminal::Completed(answer),
        PromptEnd::Stopped(StopCause::Timeout) => RunTerminal::Timeout,
        PromptEnd::Stopped(StopCause::Interrupted(signal)) => RunTerminal::Interrupted(signal),
        // Nothing but a stop cancels `rho run`'s prompt.
        PromptEnd::Stopped(StopCause::Cancelled) => {
            RunTerminal::Failed(anyhow::anyhow!("rho run was cancelled"))
        }
        PromptEnd::Failed(error) => RunTerminal::Failed(error),
    }
}

fn emit_and_exit_terminal(
    terminal: RunTerminal,
    jsonl: &mut Option<JsonlAdapter>,
    has_reporter: bool,
    messages: &TerminalMessages,
) -> anyhow::Result<()> {
    match terminal {
        RunTerminal::Timeout => {
            emit_stopped(jsonl, TerminalReason::Timeout)?;
            Err(AutomationExit::new(124, TerminalReason::Timeout, TIMEOUT_MESSAGE).into())
        }
        RunTerminal::Interrupted(signal) => {
            emit_stopped(jsonl, TerminalReason::Interrupted)?;
            Err(AutomationInterrupted::new(signal).into())
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
            if reason == TerminalReason::ProviderError {
                print_provider_diagnostic(&error);
            }
            if reason != TerminalReason::OutputError {
                emit_failure(jsonl, reason, &error, messages)?;
            }
            let message = messages.for_error(reason, &error);
            Err(AutomationExit::new(code, reason, message).into())
        }
    }
}

/// Shows the provider's own failure details to the user running `rho run`.
///
/// The SDK reserves diagnostics for direct display, so they go to stderr only,
/// never into JSONL or the exit message that automation consumes.
/// Authentication failures never reach this, because their diagnostics may
/// echo credentials.
fn print_provider_diagnostic(error: &anyhow::Error) {
    let diagnostic = error.chain().find_map(|cause| match cause.downcast_ref() {
        Some(rho_sdk::Error::Provider(provider)) => provider.diagnostic(),
        _ => None,
    });
    if let Some(diagnostic) = diagnostic {
        eprintln!("provider diagnostic: {diagnostic}");
    }
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
