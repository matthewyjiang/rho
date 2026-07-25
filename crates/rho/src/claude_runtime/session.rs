//! Execute a `runtime: claude-cli` delegated run via `claude -p`.

use std::{process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::watch,
};

use rho_tools::cancellation::RunCancellation;

#[cfg(test)]
use crate::subagent;

use crate::{
    agent::PromptPolicy,
    permission::PermissionMode,
    subagent::RunStatus,
    tools::process::{prepare_child_command, ProcessTree},
};

use super::{
    auth::{self, ClaudeAuthError, ClaudeAuthStatus},
    executable::{self, ClaudeExecutable},
    line_decoder::{claude_ndjson_line_decoder, LineDecodeError},
    persist::StatusSink,
    spawn::{self, ClaudeSpawnPlan, ClaudeSpawnRequest},
    stream::{StreamEffect, StreamMapper, TerminalResult},
};

pub(crate) use super::persist::ClaudeRunIdentity;

/// Inputs for one Claude CLI subagent run, including bound runtime values.
///
/// `AgentExecutor` builds this directly after typed binding; tests build the
/// same shape and fill [`Self::overrides`].
pub(crate) struct ClaudeSessionRequest {
    /// Agent system prompt policy. The only definition field a spawn needs.
    pub(crate) system_prompt: PromptPolicy,
    pub(crate) identity: ClaudeRunIdentity,
    /// Bound Claude `--model`. `None` means omit the flag.
    pub(crate) model: Option<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    /// Exact `--max-turns` value. Always set from the configured/definition step
    /// cap at bind/launch time; never recomputed inside the session adapter.
    pub(crate) max_turns: u64,
    /// Claude `--effort` from definition `reasoning:`. `None` omits the flag.
    pub(crate) effort: Option<&'static str>,
    pub(crate) prompt: String,
    pub(crate) output_file: std::path::PathBuf,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// When set, the launcher already force-replaced `result.json` with this
    /// Starting status. The sink continues from it instead of rewriting.
    pub(crate) started_status: Option<RunStatus>,
    pub(crate) overrides: ClaudeSessionOverrides,
}

/// Seams a session may replace. Production leaves every field unset.
#[derive(Default)]
pub(crate) struct ClaudeSessionOverrides {
    /// Claude binary to run instead of the resolved one.
    pub(crate) executable: Option<ClaudeExecutable>,
    /// Auth preflight result. When set, production `auth::query` is not called.
    pub(crate) auth_status: Option<Result<ClaudeAuthStatus, ClaudeAuthError>>,
    /// Rate-limit cache path. Tests inject a temp path so settle never touches
    /// the host default cache.
    pub(crate) rate_limit_state_path: Option<std::path::PathBuf>,
}

struct OwnedChild {
    child: Child,
    tree: ProcessTree,
}

impl OwnedChild {
    fn spawn(mut command: tokio::process::Command) -> Result<Self, std::io::Error> {
        prepare_child_command(&mut command);
        // Linux can return ETXTBSY when a just-written executable is still open
        // for write (or still being closed) under parallel test load. Retry with
        // cooperative yields only - no timed sleeps.
        let child = {
            let mut attempts = 0;
            loop {
                match command.spawn() {
                    Ok(child) => break child,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                            && attempts < 32 =>
                    {
                        attempts += 1;
                        std::thread::yield_now();
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let tree = match ProcessTree::attach(&child) {
            Ok(tree) => tree,
            Err(error) => {
                // Attach failed: best-effort kill the lone process.
                let mut child = child;
                let _ = child.start_kill();
                return Err(std::io::Error::other(error));
            }
        };
        Ok(Self { child, tree })
    }

    async fn terminate(&mut self) {
        self.tree
            .terminate(&mut self.child, Duration::from_millis(200))
            .await;
    }

    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait().await;
        // Ensure any leftover group members are cleaned after the leader exits.
        self.tree.kill();
        status
    }

    fn stdin(&mut self) -> Option<tokio::process::ChildStdin> {
        self.child.stdin.take()
    }

    fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.stdout.take()
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.tree.kill();
        let _ = self.child.start_kill();
    }
}

/// Run one Claude CLI session to completion, writing the subagent contract.
pub(crate) async fn run_session(mut request: ClaudeSessionRequest) -> anyhow::Result<()> {
    if request.identity.model.is_none() {
        request.identity.model = request.model.clone();
    }
    let mut sink = match request.started_status.take() {
        Some(status) => StatusSink::continue_from(
            request.output_file.clone(),
            status,
            &request.prompt,
            request.status_tx.take(),
            request.overrides.rate_limit_state_path.clone(),
        )?,
        None => StatusSink::new(
            request.output_file.clone(),
            &request.identity,
            &request.prompt,
            request.status_tx.take(),
            request.overrides.rate_limit_state_path.clone(),
        )?,
    };
    let outcome = drive_session(&mut request, &mut sink).await;
    settle(sink, outcome).await;
    Ok(())
}

/// What one session decided, before any terminal artifact was written.
enum SessionOutcome {
    /// Cancellation observed. The reason becomes the stop activity.
    ///
    /// When a terminal `result` already arrived, keep it so settle can still
    /// apply turns/usage/cost while reporting cancelled state.
    Cancelled {
        reason: &'static str,
        pending: Option<Box<TerminalResult>>,
    },
    /// Setup, stdin, or stream failure. Ignored when the stream already
    /// published a terminal state.
    Failed(String),
    /// The child ran and was reaped. Final Ok/Error combines pending terminal
    /// metadata with the exit status.
    Exited {
        pending: Option<Box<TerminalResult>>,
        status: std::process::ExitStatus,
        log_tail: String,
    },
}

/// Write exactly one terminal artifact for `outcome`.
///
/// Every exit path in [`drive_session`] funnels through here, so "one terminal
/// write" is structural instead of repeated per branch.
async fn settle(mut sink: StatusSink, outcome: SessionOutcome) {
    match outcome {
        SessionOutcome::Cancelled { reason, pending } => {
            sink.stop(reason, pending.as_deref()).await
        }
        SessionOutcome::Failed(error) => sink.fail(error).await,
        SessionOutcome::Exited {
            pending,
            status,
            log_tail,
        } => {
            // Protocol type:error is pending metadata only; final Failed/Completed
            // is chosen here after exit. Leave already-terminal state alone.
            if !sink.status().state.is_terminal() {
                match decide_final_outcome(pending.as_deref(), status, &log_tail) {
                    FinalOutcome::Success(terminal) => {
                        sink.finalize_success_from_stream(&terminal).await;
                    }
                    FinalOutcome::Failure {
                        terminal,
                        detail,
                        prefer_detail,
                    } => {
                        sink.finalize_failure_from_stream(terminal.as_ref(), detail, prefer_detail)
                            .await;
                    }
                }
            }
        }
    }
}

/// Preflight, spawn, and drain one Claude run without writing terminal state.
async fn drive_session(
    request: &mut ClaudeSessionRequest,
    sink: &mut StatusSink,
) -> SessionOutcome {
    if request.cancellation.is_cancelled() {
        return SessionOutcome::Cancelled {
            reason: "cancelled before execution",
            pending: None,
        };
    }
    match prepare_launch(request).await {
        Ok(launch) => run_child(request, sink, launch).await,
        Err(error) => SessionOutcome::Failed(error),
    }
}

/// Everything needed to spawn, resolved before the child exists.
struct Launch {
    executable: ClaudeExecutable,
    plan: ClaudeSpawnPlan,
    spawn_args: Vec<std::ffi::OsString>,
    log_path: std::path::PathBuf,
    log_file: std::fs::File,
}

/// Check auth, resolve the binary, and materialize argv plus the log file.
///
/// Every failure is already user-facing text; the caller turns it into
/// [`RunState::Error`](crate::subagent::RunState::Error).
async fn prepare_launch(request: &mut ClaudeSessionRequest) -> Result<Launch, String> {
    // An unauthenticated claude may block rather than exit, so preflight first.
    let auth_result = match request.overrides.auth_status.take() {
        Some(result) => result,
        None => auth::query().await,
    };
    match auth_result {
        Ok(status) if status.logged_in => {}
        Ok(_) => return Err("claude code: not signed in - run /login claude-code".into()),
        Err(ClaudeAuthError::BinaryMissing) => {
            return Err(ClaudeAuthError::BinaryMissing.to_string())
        }
        Err(error) => return Err(format!("claude code: auth preflight failed: {error}")),
    }

    let executable = match request.overrides.executable.take() {
        Some(executable) => executable,
        None => executable::resolve().map_err(|error| error.to_string())?,
    };

    let plan = spawn::build_spawn_plan(&ClaudeSpawnRequest {
        system_prompt: request.system_prompt.clone(),
        model: request.model.clone(),
        tools: request.tools.clone(),
        inherit_claude_config: request.inherit_claude_config,
        permission_mode: request.permission_mode,
        cwd: request.cwd.clone(),
        max_turns: request.max_turns,
        effort: request.effort,
    })
    .map_err(|error| error.to_string())?;

    // Materialize the system prompt next to the status file (kept as a run
    // artifact). File flags keep multiline prompt bytes out of shell/cmd argv.
    let spawn_args = spawn::finalize_spawn_args(&plan, &request.output_file)
        .map_err(|error| error.to_string())?;

    let log_path = spawn::log_path(&request.output_file);
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .map_err(|error| format!("could not open claude log file: {error}"))?
        .into_std()
        .await;

    Ok(Launch {
        executable,
        plan,
        spawn_args,
        log_path,
        log_file,
    })
}

/// Spawn the child and drain it, leaving no live process tree behind.
async fn run_child(
    request: &ClaudeSessionRequest,
    sink: &mut StatusSink,
    launch: Launch,
) -> SessionOutcome {
    let Launch {
        executable,
        plan,
        spawn_args,
        log_path,
        log_file,
    } = launch;

    // Typed fallible builder: Windows shim validation becomes RunState::Error
    // before spawn instead of a generic I/O failure at CreateProcess.
    let mut command = match executable.try_command(&spawn_args) {
        Ok(command) => command,
        Err(error) => return SessionOutcome::Failed(error.to_string()),
    };
    command
        .current_dir(&plan.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log_file)
        .kill_on_drop(true);

    let mut child = match OwnedChild::spawn(command) {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SessionOutcome::Failed(ClaudeAuthError::BinaryMissing.to_string());
        }
        Err(error) => {
            return SessionOutcome::Failed(format!(
                "claude code: failed to spawn `{}`: {error}",
                executable.display()
            ));
        }
    };

    let outcome = drain_child(request, sink, &mut child, &log_path).await;
    // Only a reaped exit guarantees the tree is gone; every other outcome
    // leaves the child mid-protocol.
    if !matches!(outcome, SessionOutcome::Exited { .. }) {
        child.terminate().await;
    }
    outcome
}

/// Write the prompt, map stdout, and wait for exit.
async fn drain_child(
    request: &ClaudeSessionRequest,
    sink: &mut StatusSink,
    child: &mut OwnedChild,
    log_path: &std::path::Path,
) -> SessionOutcome {
    sink.mark_running();

    let Some(stdout) = child.stdout() else {
        return SessionOutcome::Failed("claude code: child stdout was not captured".into());
    };

    // Prompt on stdin so shell metacharacters cannot break the command line.
    // Write stdin concurrently with the stdout drain: a child that emits enough
    // output before consuming stdin would otherwise fill the pipe and deadlock
    // if we awaited the full prompt write first.
    let stdin = child.stdin();
    let prompt = request.prompt.clone();
    let stdin_write = async move {
        let Some(mut stdin) = stdin else {
            return Ok(());
        };
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await?;
        Ok::<(), std::io::Error>(())
    };
    tokio::pin!(stdin_write);

    let mut stdout = BufReader::new(stdout);
    let mut decoder = claude_ndjson_line_decoder();
    let mut mapper = StreamMapper::new();
    let mut pending_terminal: Option<TerminalResult> = None;
    let mut stream_error: Option<String> = None;
    let mut stdin_error: Option<String> = None;
    let mut stdin_done = false;
    let mut stdout_done = false;
    let mut chunk = vec![0_u8; 8 * 1024];

    loop {
        if stdin_done && stdout_done {
            break;
        }
        tokio::select! {
            biased;
            () = request.cancellation.cancelled() => {
                // Dropping the pinned stdin future closes ChildStdin; the caller
                // reaps the tree so nothing is left orphaned.
                return SessionOutcome::Cancelled {
                    reason: "cancelled",
                    pending: pending_terminal.map(Box::new),
                };
            }
            result = &mut stdin_write, if !stdin_done => {
                stdin_done = true;
                if let Err(error) = result {
                    stdin_error = Some(format!(
                        "claude code: failed to write prompt to stdin: {error}"
                    ));
                    break;
                }
            }
            read = stdout.read(&mut chunk), if !stdout_done => {
                match read {
                    Ok(0) => {
                        stdout_done = true;
                    }
                    Ok(n) => {
                        decoder.push(&chunk[..n]);
                        loop {
                            let line = match decoder.next_line() {
                                Ok(Some(line)) => line.to_string(),
                                Ok(None) => break,
                                Err(error) => {
                                    stream_error = Some(format_line_error(&error));
                                    break;
                                }
                            };
                            apply_stream_line(
                                &mut mapper,
                                sink,
                                &mut pending_terminal,
                                &line,
                            );
                        }
                        if stream_error.is_some() {
                            break;
                        }
                    }
                    Err(error) => {
                        stream_error = Some(format!(
                            "claude code: failed reading stdout: {error}"
                        ));
                        break;
                    }
                }
            }
        }
    }

    // Stdin write failures take precedence over later stream noise: the child
    // often exits uncleanly once its stdin pipe is dropped mid-protocol.
    if let Some(error) = stdin_error {
        return SessionOutcome::Failed(error);
    }

    if stream_error.is_none() {
        match decoder.finish() {
            Ok(Some(line)) => {
                apply_stream_line(&mut mapper, sink, &mut pending_terminal, line);
            }
            Ok(None) => {}
            Err(error) => {
                stream_error = Some(format_line_error(&error));
            }
        }
    }

    if let Some(error) = stream_error {
        return SessionOutcome::Failed(error);
    }

    // After stdout EOF, wait for the process while honoring cancellation. A hang
    // here would strand the full tree after the child closed stdout and slept.
    let exit_status = tokio::select! {
        biased;
        () = request.cancellation.cancelled() => {
            return SessionOutcome::Cancelled {
                reason: "cancelled",
                pending: pending_terminal.map(Box::new),
            };
        }
        status = child.wait() => status,
    };

    match exit_status {
        Ok(status) => SessionOutcome::Exited {
            pending: pending_terminal.map(Box::new),
            status,
            log_tail: read_log_tail(log_path).await,
        },
        Err(error) => {
            SessionOutcome::Failed(format!("claude code: failed waiting for child: {error}"))
        }
    }
}

enum FinalOutcome {
    Success(TerminalResult),
    Failure {
        terminal: Option<TerminalResult>,
        detail: String,
        /// Prefer `detail` over stream result/error text (nonzero exit, max-turns).
        prefer_detail: bool,
    },
}

/// Final truth: only explicit valid success + exit 0 + no prior stream error
/// becomes Ok. Any failure/invalid/nonzero/missing result becomes Error.
///
/// The stream mapper never emits Completed/Failed for `result` or protocol
/// `type:error` messages. Session writes exactly one terminal attachment here
/// after process exit, combining pending terminal metadata with exit status.
fn decide_final_outcome(
    pending: Option<&TerminalResult>,
    exit_status: std::process::ExitStatus,
    log_tail: &str,
) -> FinalOutcome {
    if !exit_status.success() {
        if spawn::looks_like_max_turns_unsupported(log_tail) {
            return FinalOutcome::Failure {
                terminal: pending.cloned(),
                detail: "claude code: this claude binary rejected --max-turns; upgrade Claude Code or remove the turn cap".into(),
                prefer_detail: true,
            };
        }
        let detail = if log_tail.is_empty() {
            format!("claude code: process exited with {exit_status}")
        } else {
            format!("claude code: process exited with {exit_status}: {log_tail}")
        };
        return FinalOutcome::Failure {
            terminal: pending.cloned(),
            detail,
            prefer_detail: true,
        };
    }

    match pending {
        Some(terminal) if terminal.classification.is_success() => {
            FinalOutcome::Success(terminal.clone())
        }
        Some(terminal)
            if terminal.classification.is_failure() || terminal.classification.is_invalid() =>
        {
            let detail = terminal
                .error
                .clone()
                .or_else(|| terminal.result_text.clone())
                .unwrap_or_else(|| "claude code: terminal result was not success".into());
            FinalOutcome::Failure {
                terminal: Some(terminal.clone()),
                detail,
                prefer_detail: false,
            }
        }
        Some(terminal) => FinalOutcome::Failure {
            terminal: Some(terminal.clone()),
            detail: "claude code: terminal result classification was not success".into(),
            prefer_detail: true,
        },
        None => FinalOutcome::Failure {
            terminal: None,
            detail: "claude code: stream ended without a terminal result message; see log.txt for details".into(),
            prefer_detail: true,
        },
    }
}

fn apply_stream_line(
    mapper: &mut StreamMapper,
    sink: &mut StatusSink,
    pending_terminal: &mut Option<TerminalResult>,
    line: &str,
) {
    for effect in mapper.push_line(line) {
        if let StreamEffect::Terminal(terminal) = &effect {
            // Later terminals (for example a final `result`) replace earlier
            // pending protocol-error metadata.
            *pending_terminal = Some(terminal.clone());
        }
        sink.apply_effect(effect);
    }
}

fn format_line_error(error: &LineDecodeError) -> String {
    match error {
        LineDecodeError::InvalidUtf8(_) => {
            format!("claude code: malformed UTF-8 on stream-json stdout: {error}")
        }
        LineDecodeError::LineTooLong { .. } => {
            format!("claude code: oversize stream-json line: {error}")
        }
    }
}

async fn read_log_tail(path: &std::path::Path) -> String {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return String::new();
    };
    let trimmed = contents.trim();
    if trimmed.len() <= 400 {
        return trimmed.to_string();
    }
    let cut = trimmed.len() - 400;
    let boundary = (cut..trimmed.len())
        .find(|index| trimmed.is_char_boundary(*index))
        .unwrap_or(cut);
    format!("…{}", &trimmed[boundary..])
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
