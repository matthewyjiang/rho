//! `rho run`: one prompt on a saved session with no person watching, its
//! answer written as text or its events as the versioned JSONL stream (see
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

use rho_protocol::{SessionEvent, TurnOutcome as WireOutcome};
use rho_sdk::UserInput;

use super::{
    agent_binding::BoundAgent,
    automation_protocol::{JsonlStream, TerminalReason},
    headless_host::{self, HeadlessView, PromptEnd, RunReporter, StopCause, Stops},
    session_commands::{self, BuiltInCommand, CommandScope, CommandSet, PromptCommand},
    session_events::outcome,
    session_runtime::{
        CommandRejected, HostCapabilities, Logouts, PromptScope, RunShape, SessionBuild,
        SessionLaunch, SessionOpening, SessionRole, SessionRuntime, Turn, TurnOutcome,
        UnattendedApprovals,
    },
    shutdown_signal::{ShutdownSignal, ShutdownSignals},
};
use crate::{
    cli::{Command, OutputFormat},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    herdr::{HerdrReporter, HerdrState},
    session::SessionPersistence,
};

mod terminal;

/// Error returned after an automation run has cleaned up and selected a stable exit code.
#[derive(Clone, Debug)]
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
    /// Whether the run's session is saved; `--no-save` makes it ephemeral.
    pub persistence: SessionPersistence,
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

/// Writes the JSONL stream of a run that failed to start with `message`:
/// only its `run_ended`.
pub(super) fn emit_startup_failure(message: &str) -> anyhow::Result<()> {
    JsonlStream::default()
        .end(TerminalReason::ConfigurationError, 2, Some(message))
        .map_err(jsonl_write_failure)
}

pub(super) async fn run(prompt_text: String, startup: Startup<'_>) -> anyhow::Result<()> {
    let mut jsonl = (startup.output == OutputFormat::Jsonl).then(JsonlStream::default);
    // The bound agent may pin its own provider and auth; that is what runs.
    let selection = startup.agent.rho_config().unwrap_or(startup.config);
    let selection = Selection {
        provider: &selection.provider,
        auth: &selection.auth,
    };
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
            )
        })
        .transpose();
    let mut reporter = match reporter_result {
        Ok(reporter) => reporter,
        Err(error) => {
            let exit = AutomationExit::new(1, TerminalReason::OutputError, error.to_string());
            end_jsonl(&mut jsonl, || unreachable!("no turn began"), &exit)?;
            return Err(exit.into());
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
    emit_and_exit_terminal(terminal, &mut jsonl, reporter.is_some(), selection)
}

/// Runs the prompt on a new session saved as `rho run`'s, unless
/// `--no-save` made it ephemeral, until it ends or `stops` stops it, and
/// tears the session down. Returns how the prompt ended. Person-facing
/// listings leave the session out, so it opens by id. It is never titled: a
/// model call per scripted run is not worth it.
async fn run_session(
    prompt_text: String,
    startup: &Startup<'_>,
    stops: &mut Stops,
    reporter: Option<&mut RunReporter>,
    jsonl: Option<&mut JsonlStream>,
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
    let mut view = HeadlessView::new();
    if let Some(reporter) = reporter {
        view = view.reporting_to(reporter);
    }
    if let Some(stream) = jsonl {
        view = view.streaming_jsonl(stream);
    }
    let herdr = &startup.herdr;
    let open = async {
        let runtime =
            SessionRuntime::open_new(opening, rho_sdk::SessionId::new(), startup.persistence)
                .await?;
        let _ = herdr.report_state(HerdrState::Working, None, None).await;
        anyhow::Ok(runtime)
    };
    let end = headless_host::run_prompt(
        stops,
        &mut view,
        startup.config,
        open,
        |runtime, view, scope, process_config| {
            Box::pin(run_text(runtime, view, scope, prompt_text, process_config))
        },
    )
    .await;
    let _ = herdr.report_state(HerdrState::Idle, None, None).await;
    let _ = herdr.release().await;
    // A JSONL write that failed already failed the turn; the stream is gone.
    match view.take_output_failure() {
        Some(message) => {
            let failed = AutomationExit::new(1, TerminalReason::OutputError, message);
            PromptEnd::Failed(failed.into())
        }
        None => end,
    }
}

/// Runs `text` within `prompt`: its slash command when it starts with one
/// `rho run` runs, else a model turn on it. A command `rho run` cannot run
/// fails the prompt as a configuration error.
async fn run_text(
    runtime: &mut SessionRuntime,
    view: &mut HeadlessView<'_>,
    prompt: &PromptScope,
    text: String,
    process_config: &Config,
) -> anyhow::Result<TurnOutcome> {
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
            view.prompt(&input);
            let turn = Turn {
                display: input,
                model,
                calls,
            };
            runtime
                .run_prompt_turn(view, prompt, turn, process_config)
                .await
        }
        PromptCommand::RunAsAgent(task) => {
            runtime
                .rebind_agent(prompt, &task, process_config)
                .await
                .map_err(|error| match error.downcast::<CommandRejected>() {
                    Ok(CommandRejected(message)) => rejected(message),
                    Err(error) => error,
                })?;
            // The rebound agent has tools of its own.
            view.bind(runtime);
            view.prompt(&input);
            // The command stays the shown message; the model gets the task.
            let turn = Turn {
                display: input,
                model: task.input,
                calls: Vec::new(),
            };
            runtime
                .run_prompt_turn(view, prompt, turn, process_config)
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

/// The exit of a run whose JSONL stream could not be written.
fn jsonl_write_failure(error: io::Error) -> anyhow::Error {
    AutomationExit::new(
        1,
        TerminalReason::OutputError,
        format!("could not write JSONL output: {error}"),
    )
    .into()
}

/// Ends the JSONL stream, if the run writes one, as `exit` says: the turn's
/// `turn_ended` with `outcome`, if the turn began, then `run_ended`.
fn end_jsonl(
    jsonl: &mut Option<JsonlStream>,
    outcome: impl FnOnce() -> WireOutcome,
    exit: &AutomationExit,
) -> anyhow::Result<()> {
    let Some(stream) = jsonl.as_mut() else {
        return Ok(());
    };
    if stream.turn_began() {
        let outcome = outcome();
        stream
            .event(&SessionEvent::TurnEnded { outcome })
            .map_err(jsonl_write_failure)?;
    }
    let message = (exit.reason != TerminalReason::Completed).then_some(exit.message.as_str());
    stream
        .end(exit.reason, exit.code, message)
        .map_err(jsonl_write_failure)
}

/// The provider and auth the run's model uses, which an authentication
/// failure names.
#[derive(Clone, Copy)]
struct Selection<'a> {
    provider: &'a str,
    auth: &'a str,
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
        PromptEnd::Stopped(StopCause::Cancelled) => {
            unreachable!("rho run's stops have no parent cancel")
        }
        PromptEnd::Failed(error) => RunTerminal::Failed(error),
    }
}

fn emit_and_exit_terminal(
    terminal: RunTerminal,
    jsonl: &mut Option<JsonlStream>,
    has_reporter: bool,
    selection: Selection<'_>,
) -> anyhow::Result<()> {
    match terminal {
        RunTerminal::Timeout => {
            let exit = AutomationExit::new(124, TerminalReason::Timeout, TIMEOUT_MESSAGE);
            end_jsonl(jsonl, || WireOutcome::Cancelled, &exit)?;
            Err(exit.into())
        }
        RunTerminal::Interrupted(signal) => {
            let interrupted = AutomationInterrupted::new(signal);
            let exit = AutomationExit::new(
                interrupted.exit_code(),
                TerminalReason::Interrupted,
                interrupted.to_string(),
            );
            end_jsonl(jsonl, || WireOutcome::Cancelled, &exit)?;
            Err(interrupted.into())
        }
        RunTerminal::MaxSteps(answer) => {
            let exit = AutomationExit::new(124, TerminalReason::MaxSteps, MAX_STEPS_MESSAGE);
            write_answer(jsonl, &answer, &exit, has_reporter)?;
            Err(exit.into())
        }
        RunTerminal::Completed(answer) => {
            let exit = AutomationExit::new(0, TerminalReason::Completed, "");
            write_answer(jsonl, &answer, &exit, has_reporter)
        }
        RunTerminal::Failed(error) => {
            let exit = terminal::failure(&error, selection.provider, selection.auth);
            if exit.reason == TerminalReason::ProviderError {
                print_provider_diagnostic(&error);
            }
            // A failed JSONL write broke the stream; nothing more goes out.
            if exit.reason != TerminalReason::OutputError {
                let outcome = || WireOutcome::Failed {
                    error: outcome::turn_error(&error, selection.provider, selection.auth),
                };
                end_jsonl(jsonl, outcome, &exit)?;
            }
            Err(exit.into())
        }
    }
}

/// Writes the run's `answer`: as the end of its JSONL stream, if it writes
/// one, else as text.
fn write_answer(
    jsonl: &mut Option<JsonlStream>,
    answer: &rho_sdk::RunOutcome,
    exit: &AutomationExit,
    has_reporter: bool,
) -> anyhow::Result<()> {
    if jsonl.is_some() {
        end_jsonl(jsonl, || outcome::finished(answer), exit)
    } else {
        write_text_answer(answer, has_reporter)
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
