//! Shell commands an embedding host runs in its own terminals.
//!
//! An editor host can show a terminal live in a tool call's card. When it
//! supplies [`HostTerminals`], the shell tool runs each approved command in
//! one instead of spawning a local child. Everything else stays with the
//! tool: argument parsing, workspace and permission checks, the timeout,
//! cancellation, output limits, and the model-facing result.

use std::{path::PathBuf, time::Duration, time::Instant};

use rho_sdk::{
    tool::{OperationKind, ProcessResult, ToolMetadata, ToolProgress, ToolProgressSender},
    ExecutableSelection, ProcessEnvironment, ProcessExecution, ProcessInvocation,
};

use crate::{
    cancellation::RunCancellation,
    host::HostFuture,
    shell_process::{finished_content, timeout_error, ShellOutcome},
    tool::ToolError,
    Rendered,
};

/// Terminals the embedding host, such as an editor, runs commands in.
///
/// Implementors own each terminal's lifetime. The shell tool never releases
/// a terminal; it drops its [`HostTerminal`] handle when the call ends, and
/// the implementor releases the terminal once the host no longer shows it.
pub trait HostTerminals: Send + Sync {
    /// Starts `command` in a new terminal and returns once the host accepted
    /// it. Dropping the future abandons the request, which the host may still
    /// have carried out.
    fn create(&self, command: HostCommand) -> HostFuture<'_, Box<dyn HostTerminal>>;
}

/// A command for a host terminal: `program` with `args`, in `cwd`, with `env`
/// added to the host's environment. The host keeps at most
/// `output_byte_limit` bytes of output, dropping the oldest.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub output_byte_limit: u64,
}

impl HostCommand {
    /// `program` with `args` in `cwd`, adding no environment variables.
    pub fn new(
        program: impl Into<String>,
        args: Vec<String>,
        cwd: impl Into<PathBuf>,
        output_byte_limit: u64,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            env: Vec::new(),
            cwd: cwd.into(),
            output_byte_limit,
        }
    }
}

/// One running or finished command in a host terminal.
///
/// The shell tool may drop a pending [`Self::wait_for_exit`] when the call
/// times out or is cancelled, and then calls [`Self::kill`], which can race
/// the command's own exit; killing a command that already exited must
/// succeed or fail harmlessly.
pub trait HostTerminal: Send + Sync {
    /// The host's id for the terminal, which hosts use to show it on a card.
    fn id(&self) -> &str;

    /// Resolves when the command exits.
    fn wait_for_exit(&self) -> HostFuture<'_, HostExitStatus>;

    /// The output retained so far.
    fn output(&self) -> HostFuture<'_, HostTerminalOutput>;

    /// Stops the command. The terminal and its output stay readable.
    fn kill(&self) -> HostFuture<'_, ()>;
}

/// How a host terminal's command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostExitStatus {
    /// `None` when the command did not exit normally, such as on a signal.
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
}

impl HostExitStatus {
    pub fn new(exit_code: Option<i32>, signal: Option<String>) -> Self {
        Self { exit_code, signal }
    }
}

/// A host terminal's retained output, stdout and stderr interleaved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostTerminalOutput {
    pub output: String,
    /// Whether the host dropped the oldest output to stay within its limit.
    pub truncated: bool,
}

impl HostTerminalOutput {
    pub fn new(output: String, truncated: bool) -> Self {
        Self { output, truncated }
    }
}

/// Marks output whose beginning the host dropped.
const EARLIER_OUTPUT_DROPPED: &str = "[earlier output truncated]\n";

/// The host command for an approved `bash` execution. A plan the host
/// cannot run as approved is an error: running it locally instead could
/// bypass a sandbox the host applies to its terminals.
///
/// The command runs as it would locally, though in the host's terminal:
/// - Hosts can add environment variables but not remove them, so `env -u`
///   removes withheld ones before `bash` starts. Unsetting them in the
///   script would be too late: startup files such as `$BASH_ENV` would see
///   them, and so would `/proc/<pid>/environ`.
/// - Its stdin is empty, and pagers print instead of waiting for keys, so
///   a command that prompts fails instead of waiting on the terminal.
/// - The shell is a non-login `bash -c`: hosts such as Zed already start
///   commands from the user's shell environment.
pub(crate) fn host_command(execution: &ProcessExecution) -> Result<HostCommand, ToolError> {
    let unsupported =
        |detail: &str| ToolError::Message(format!("the host terminal cannot run {detail}"));
    let ProcessInvocation::Shell {
        executable,
        selection: ExecutableSelection::SearchPath,
        arguments,
        command,
    } = execution.invocation()
    else {
        return Err(unsupported("this process plan"));
    };
    // The approved local form is a login `bash -lc`; any other arguments
    // would make the host command differ from what was approved.
    if arguments.as_slice() != ["-lc"] {
        return Err(unsupported("this process plan"));
    }
    if executable != "bash" {
        return Err(unsupported(&format!("`{}`", executable.display())));
    }
    let withheld = match execution.environment() {
        ProcessEnvironment::InheritAll => &[][..],
        ProcessEnvironment::InheritExcept { variable_names } => variable_names.as_slice(),
        _ => return Err(unsupported("a command with a restricted environment")),
    };
    if let Some(name) = withheld
        .iter()
        .find(|name| name.is_empty() || name.contains(['=', '\0']))
    {
        return Err(unsupported(&format!(
            "a command that must not see the variable `{name}`, which `env -u` cannot name"
        )));
    }
    let mut args = withheld
        .iter()
        .flat_map(|name| ["-u".to_owned(), name.clone()])
        .collect::<Vec<_>>();
    args.extend([
        "bash".to_owned(),
        "-c".to_owned(),
        format!("exec </dev/null\n{command}"),
    ]);
    Ok(HostCommand {
        program: "env".into(),
        args,
        env: [("PAGER", "cat"), ("GIT_PAGER", "cat")]
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .into(),
        cwd: execution.working_directory().to_path_buf(),
        output_byte_limit: u64::try_from(execution.output_limits().max_output_bytes())
            .unwrap_or(u64::MAX),
    })
}

/// Runs `command` in a host terminal: announces the terminal through
/// `progress`, enforces `timeout` and `cancellation` by killing it, then
/// reads its final output. Returns the outcome and the terminal's id.
pub(crate) async fn run(
    terminals: &dyn HostTerminals,
    command: HostCommand,
    timeout: Option<Duration>,
    cancellation: &RunCancellation,
    progress: &ToolProgressSender,
) -> Result<(Rendered<ShellOutcome>, String), ToolError> {
    let max_output_bytes = usize::try_from(command.output_byte_limit).unwrap_or(usize::MAX);
    let terminal = terminals.create(command).await.map_err(|error| {
        ToolError::Message(format!(
            "could not start the command in the host terminal: {error}"
        ))
    })?;
    let id = terminal.id().to_owned();
    let _ = progress
        .send(
            ToolProgress::message(String::new()).metadata(
                ToolMetadata::new()
                    .operation(OperationKind::Execute)
                    .host_terminal(id.clone()),
            ),
        )
        .await;
    let started = Instant::now();
    // A command that already exited keeps its status even when the timeout
    // is due at the same moment.
    let exit = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            let _ = terminal.kill().await;
            return Err(ToolError::Cancelled);
        }
        exit = terminal.wait_for_exit() => match exit {
            Ok(exit) => exit,
            Err(error) => {
                // The command may still be running; stop it rather than
                // leave it to run until the terminal is released.
                let _ = terminal.kill().await;
                return Err(host_failure("wait for", &error));
            }
        },
        () = tokio::time::sleep(timeout.unwrap_or(Duration::MAX)), if timeout.is_some() => {
            let _ = terminal.kill().await;
            let output = terminal.output().await.unwrap_or_default();
            let timeout = timeout.unwrap_or_default();
            let overhead = timeout_error(b"x", b"", timeout, usize::MAX).to_string().len() - 1;
            let (text, _) = retained(&output, max_output_bytes.saturating_sub(overhead));
            return Err(timeout_error(text.as_bytes(), b"", timeout, max_output_bytes));
        }
    };
    let output = terminal
        .output()
        .await
        .map_err(|error| host_failure("read the output of", &error))?;
    let elapsed = started.elapsed();
    let failure = match (exit.exit_code, &exit.signal) {
        (Some(0), _) => None,
        (Some(code), _) => Some(code.to_string()),
        (None, Some(signal)) => Some(signal.clone()),
        (None, None) => Some("signal".to_owned()),
    };
    let overhead = finished_content(failure.as_deref(), "x", "", elapsed, usize::MAX).len() - 1;
    // The result for the newest `budget` bytes of output, and whether its
    // structured outcome fits too: its JSON escapes the same output, and an
    // outcome over `max_output_bytes` makes the result limit cut the newest
    // output instead.
    let render = |budget: usize| -> Result<(String, ShellOutcome, bool), ToolError> {
        let (text, truncated) = retained(&output, budget);
        let content = finished_content(failure.as_deref(), &text, "", elapsed, max_output_bytes);
        let outcome = ShellOutcome {
            stdout: text.clone(),
            stderr: String::new(),
            exit_code: exit.exit_code,
            truncated,
            wall_time_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            process: ProcessResult::new(exit.exit_code, text, "").with_truncated(truncated),
        };
        let fits = crate::json_len(&outcome)
            .map_err(|error| ToolError::Message(format!("could not encode the result: {error}")))?
            <= max_output_bytes;
        Ok((content, outcome, fits))
    };
    let budget = max_output_bytes.saturating_sub(overhead);
    let mut kept = render(budget)?;
    if !kept.2 {
        // Escaping can expand output up to sixfold, so search for the
        // largest budget that fits rather than subtract the excess.
        let (mut fitting, mut over) = (0, budget);
        kept = render(0)?;
        while fitting + 1 < over {
            let middle = fitting + (over - fitting) / 2;
            let candidate = render(middle)?;
            if candidate.2 {
                (fitting, kept) = (middle, candidate);
            } else {
                over = middle;
            }
        }
    }
    let (content, outcome, _) = kept;
    let failed = failure.is_some();
    Ok((Rendered::new(content, outcome).failed_if(failed), id))
}

/// The output the host retained, within `budget` bytes, and whether its
/// beginning was dropped. Like the host, it keeps the newest output: when
/// the host or the budget dropped the beginning, a marker replaces it.
fn retained(output: &HostTerminalOutput, budget: usize) -> (String, bool) {
    let text = &output.output;
    if !output.truncated && text.len() <= budget {
        return (text.clone(), false);
    }
    let mut start = text
        .len()
        .saturating_sub(budget.saturating_sub(EARLIER_OUTPUT_DROPPED.len()));
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (format!("{EARLIER_OUTPUT_DROPPED}{}", &text[start..]), true)
}

fn host_failure(action: &str, error: &crate::HostError) -> ToolError {
    ToolError::Message(format!("could not {action} the host terminal: {error}"))
}

#[cfg(test)]
#[path = "host_terminals_tests.rs"]
mod tests;
