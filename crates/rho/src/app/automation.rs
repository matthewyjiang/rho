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

use rho_protocol::{TurnError, TurnOutcome as WireOutcome};
use rho_sdk::UserInput;

use super::{
    agent_binding::BoundAgent,
    automation_protocol::{JsonlStream, RunEnd, TerminalReason},
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

pub(super) use terminal::outcome_exit;

/// Error returned after an automation run has cleaned up and selected a stable exit code.
#[derive(Clone, Debug)]
pub struct AutomationExit {
    end: RunEnd,
}

impl AutomationExit {
    pub(super) fn new(code: u8, reason: TerminalReason, message: impl Into<String>) -> Self {
        Self {
            end: RunEnd {
                reason,
                exit_code: code,
                message: Some(message.into()),
            },
        }
    }

    /// An invalid invocation or configuration: exit 2.
    pub(super) fn configuration(message: impl Into<String>) -> Self {
        Self::new(2, TerminalReason::ConfigurationError, message)
    }

    /// Any other failure, such as not reaching the daemon: exit 1.
    pub(super) fn other(message: impl Into<String>) -> Self {
        Self::new(1, TerminalReason::OtherError, message)
    }

    /// Returns the documented process exit code for this automation result.
    pub fn exit_code(&self) -> u8 {
        self.end.exit_code
    }
}

impl fmt::Display for AutomationExit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.end.message.as_deref().unwrap_or_default())
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

/// Writes the JSONL stream of a run that failed to start with `exit`: only
/// its `run_ended`. Returns the exit, unless the stream could not be
/// written, which is an output error.
pub(super) fn emit_startup_failure(exit: AutomationExit) -> AutomationExit {
    end_failed(&mut JsonlStream::stdout(), exit)
}

/// Ends `stream` as a run that failed with `exit` did, and returns the
/// exit, unless the stream could not be written, which is an output error.
fn end_failed(stream: &mut JsonlStream, exit: AutomationExit) -> AutomationExit {
    end_stream(stream, Err(exit)).expect_err("a failed run ends failed")
}

/// Ends `stream` as a run that ended with `exit` did, and returns the exit,
/// unless the stream could not be written, which is an output error.
pub(super) fn end_stream(
    stream: &mut JsonlStream,
    exit: Result<(), AutomationExit>,
) -> Result<(), AutomationExit> {
    let end = match &exit {
        Ok(()) => RunEnd::completed(),
        Err(exit) => exit.end.clone(),
    };
    stream.end(&end);
    stream_written(stream)?;
    exit
}

/// The output error of a stream that could not be written, if it could not.
pub(super) fn stream_written(stream: &JsonlStream) -> Result<(), AutomationExit> {
    stream
        .failure()
        .map_or(Ok(()), |failure| Err(output_error(failure)))
}

/// The exit of a run whose output failed with `failure`.
pub(super) fn output_error(failure: &str) -> AutomationExit {
    AutomationExit::new(1, TerminalReason::OutputError, failure)
}

/// The exit of a run whose output could not be written.
pub(super) fn write_failed(error: io::Error) -> AutomationExit {
    output_error(&format!("could not write output: {error}"))
}

pub(super) async fn run(prompt_text: String, startup: Startup<'_>) -> anyhow::Result<()> {
    let mut jsonl = (startup.output == OutputFormat::Jsonl).then(JsonlStream::stdout);
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
            let exit = match &mut jsonl {
                Some(stream) => end_failed(stream, exit),
                None => exit,
            };
            return Err(exit.into());
        }
    };

    let terminal = run_session(
        prompt_text,
        &startup,
        &mut stops,
        reporter.as_mut(),
        jsonl.as_mut(),
    )
    .await;
    if let Some(reporter) = reporter.as_mut() {
        match &terminal {
            RunTerminal::Finished { answer, outcome } => match outcome {
                WireOutcome::MaxSteps { .. } => reporter.finish_stopped(),
                _ => reporter.finish_ok(answer),
            },
            RunTerminal::Timeout | RunTerminal::Interrupted(_) => reporter.finish_stopped(),
            RunTerminal::Failed { error, .. } => reporter.finish_error(error),
        }
    }
    emit_and_exit_terminal(terminal, jsonl.as_mut(), reporter.is_some())
}

/// Runs the prompt on a new session saved as `rho run`'s, unless
/// `--no-save` made it ephemeral, until it ends or `stops` stops it, and
/// tears the session down. Returns how the prompt ended, which ends the
/// JSONL stream's turn. Person-facing
/// listings leave the session out, so it opens by id. It is never titled: a
/// model call per scripted run is not worth it.
async fn run_session(
    prompt_text: String,
    startup: &Startup<'_>,
    stops: &mut Stops,
    reporter: Option<&mut RunReporter>,
    jsonl: Option<&mut JsonlStream>,
) -> RunTerminal {
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
    let end = match view.output_failure() {
        Some(message) => PromptEnd::Failed(output_error(&message).into()),
        None => end,
    };
    // The bound agent may pin its own provider and auth; that is what runs.
    let selection = startup.agent.rho_config().unwrap_or(startup.config);
    let terminal = RunTerminal::of(end, selection);
    view.end_turn(terminal.turn_outcome());
    terminal
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
    let rejected =
        |message: String| -> anyhow::Error { AutomationExit::configuration(message).into() };
    view.prompt(&input);
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
    result.map_err(|error| write_failed(error).into())
}

const TIMEOUT_MESSAGE: &str = "rho run timed out";

/// How the run's prompt ended, decided once for the JSONL stream, the
/// status file, and the exit.
enum RunTerminal {
    /// The run finished: `outcome` is answered, or at its step limit.
    Finished {
        answer: rho_sdk::RunOutcome,
        outcome: WireOutcome,
    },
    Timeout,
    Interrupted(ShutdownSignal),
    /// The run failed with `error`, which clients are told as `turn`.
    Failed {
        error: anyhow::Error,
        turn: TurnError,
    },
}

impl RunTerminal {
    /// The terminal of a prompt that ended as `end`, in a session whose
    /// model is `selection`'s.
    fn of(end: PromptEnd, selection: &Config) -> Self {
        match end {
            PromptEnd::Finished(answer) => Self::Finished {
                outcome: outcome::finished(&answer),
                answer,
            },
            PromptEnd::Stopped(StopCause::Timeout) => Self::Timeout,
            PromptEnd::Stopped(StopCause::Interrupted(signal)) => Self::Interrupted(signal),
            PromptEnd::Stopped(StopCause::Cancelled) => {
                unreachable!("rho run's stops have no parent cancel")
            }
            PromptEnd::Failed(error) => Self::Failed {
                turn: terminal::turn_error(&error, &selection.provider, &selection.auth),
                error,
            },
        }
    }

    /// How the turn ended, as the JSONL stream's `turn_ended` says: a
    /// timeout or signal cancelled it.
    fn turn_outcome(&self) -> WireOutcome {
        match self {
            Self::Finished { outcome, .. } => outcome.clone(),
            Self::Timeout | Self::Interrupted(_) => WireOutcome::Cancelled,
            Self::Failed { turn, .. } => WireOutcome::Failed {
                error: turn.clone(),
            },
        }
    }
}

/// Ends the run: writes its answer as text unless it streams JSONL, ends
/// the stream with `run_ended`, and returns the exit.
fn emit_and_exit_terminal(
    terminal: RunTerminal,
    jsonl: Option<&mut JsonlStream>,
    has_reporter: bool,
) -> anyhow::Result<()> {
    let (end, exit) = match terminal {
        RunTerminal::Finished { answer, outcome } => {
            if jsonl.is_none() {
                write_text_answer(&answer, has_reporter)?;
            }
            match terminal::outcome_exit(&outcome) {
                Ok(()) => (RunEnd::completed(), Ok(())),
                Err(exit) => exited(exit),
            }
        }
        RunTerminal::Timeout => exited(AutomationExit::new(
            124,
            TerminalReason::Timeout,
            TIMEOUT_MESSAGE,
        )),
        RunTerminal::Interrupted(signal) => {
            let interrupted = AutomationInterrupted::new(signal);
            let end = RunEnd {
                reason: TerminalReason::Interrupted,
                exit_code: interrupted.exit_code(),
                message: Some(interrupted.to_string()),
            };
            (end, Err(interrupted.into()))
        }
        RunTerminal::Failed { error, turn } => {
            let exit = terminal::failure(&error, turn);
            if exit.end.reason == TerminalReason::ProviderError {
                print_provider_diagnostic(&error);
            }
            exited(exit)
        }
    };
    if let Some(stream) = jsonl {
        stream.end(&end);
        // A stream that could not be written ends the run as an output
        // error, whatever the run's own end.
        stream_written(stream)?;
    }
    exit
}

/// The `run_ended` record and the exit of a run that ends with `exit`.
fn exited(exit: AutomationExit) -> (RunEnd, anyhow::Result<()>) {
    (exit.end.clone(), Err(exit.into()))
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

pub(super) fn prompt_from_stdin(parts: Vec<String>, read_stdin: bool) -> anyhow::Result<String> {
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
