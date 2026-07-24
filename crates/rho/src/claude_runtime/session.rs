//! Execute a `runtime: claude-cli` delegated run via `claude -p`.

use std::{process::Stdio, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::watch,
};

use rho_tools::cancellation::RunCancellation;

use crate::{
    agent::AgentDefinition,
    permission::PermissionMode,
    subagent::RunStatus,
    tools::process::{prepare_child_command, ProcessTree},
};

#[cfg(test)]
use crate::subagent;

use super::{
    auth::{self, ClaudeAuthError, ClaudeAuthStatus},
    executable::{self, ClaudeExecutable},
    line_decoder::{LineDecodeError, LineDecoder},
    persist::{self, StatusSink},
    spawn::{self, ClaudeSpawnRequest},
    stream::{StreamEffect, StreamMapper, TerminalResult},
};

pub(crate) use persist::ClaudeRunIdentity;

/// Inputs for one Claude CLI subagent run, including bound runtime values.
pub(crate) struct ClaudeSessionRequest {
    pub(crate) definition: AgentDefinition,
    pub(crate) identity: ClaudeRunIdentity,
    /// Bound Claude `--model`. `None` means omit the flag.
    pub(crate) model: Option<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    /// Exact `--max-turns` value from the bound step budget.
    pub(crate) max_turns: u64,
    pub(crate) prompt: String,
    pub(crate) output_file: std::path::PathBuf,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// Optional test/production override for the Claude binary.
    pub(crate) executable: Option<ClaudeExecutable>,
    /// Optional auth preflight override. When set, production `auth::query` is
    /// not called. Useful for fake-child tests.
    pub(crate) auth_status: Option<Result<ClaudeAuthStatus, ClaudeAuthError>>,
    /// Optional persistence test hooks (stall/fail writer).
    #[cfg(test)]
    pub(crate) persist_hooks: Option<persist::PersistHooks>,
}

/// Bound launch package from `AgentExecutor` after typed binding.
pub(crate) struct ClaudeBoundLaunch {
    pub(crate) definition: Arc<AgentDefinition>,
    pub(crate) identity: ClaudeRunIdentity,
    pub(crate) model: Option<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    pub(crate) permission_mode: PermissionMode,
    /// Exact `--max-turns` value. Always set from the configured/definition step
    /// cap at bind/launch time; never recomputed inside the session adapter.
    pub(crate) max_turns: u64,
    pub(crate) prompt: String,
    pub(crate) output_file: std::path::PathBuf,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: watch::Sender<RunStatus>,
}

struct OwnedChild {
    child: Child,
    tree: ProcessTree,
}

impl OwnedChild {
    fn spawn(mut command: tokio::process::Command) -> Result<Self, std::io::Error> {
        prepare_child_command(&mut command);
        let child = command.spawn()?;
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
pub(crate) async fn run_session(request: ClaudeSessionRequest) -> anyhow::Result<()> {
    let mut identity = request.identity;
    if identity.model.is_none() {
        identity.model = request.model.clone();
    }
    let mut sink = {
        #[cfg(test)]
        {
            match request.persist_hooks {
                Some(hooks) => StatusSink::new_with_hooks(
                    request.output_file.clone(),
                    &identity,
                    &request.prompt,
                    request.status_tx,
                    hooks,
                )?,
                None => StatusSink::new(
                    request.output_file.clone(),
                    &identity,
                    &request.prompt,
                    request.status_tx,
                )?,
            }
        }
        #[cfg(not(test))]
        {
            StatusSink::new(
                request.output_file.clone(),
                &identity,
                &request.prompt,
                request.status_tx,
            )?
        }
    };

    if request.cancellation.is_cancelled() {
        sink.stop("cancelled before execution").await;
        sink.shutdown().await;
        return Ok(());
    }

    // Preflight auth. An unauthenticated claude may block rather than exit.
    let auth_result = match request.auth_status {
        Some(result) => result,
        None => auth::query().await,
    };
    match auth_result {
        Ok(status) if status.logged_in => {}
        Ok(_) => {
            sink.fail("claude code: not signed in - run /login claude-code")
                .await;
            sink.shutdown().await;
            return Ok(());
        }
        Err(ClaudeAuthError::BinaryMissing) => {
            sink.fail(ClaudeAuthError::BinaryMissing.to_string()).await;
            sink.shutdown().await;
            return Ok(());
        }
        Err(error) => {
            sink.fail(format!("claude code: auth preflight failed: {error}"))
                .await;
            sink.shutdown().await;
            return Ok(());
        }
    }

    let executable = match request.executable {
        Some(executable) => executable,
        None => match executable::resolve() {
            Ok(executable) => executable,
            Err(error) => {
                sink.fail(error.to_string()).await;
                sink.shutdown().await;
                return Ok(());
            }
        },
    };

    let plan = match spawn::build_spawn_plan(&ClaudeSpawnRequest {
        definition: request.definition.clone(),
        model: request.model.clone(),
        tools: request.tools.clone(),
        inherit_claude_config: request.inherit_claude_config,
        permission_mode: request.permission_mode,
        cwd: request.cwd.clone(),
        max_turns: request.max_turns,
    }) {
        Ok(plan) => plan,
        Err(error) => {
            sink.fail(error.to_string()).await;
            sink.shutdown().await;
            return Ok(());
        }
    };

    // Materialize system prompt next to the status file (kept as a run artifact).
    // File flags keep multiline prompt bytes out of shell/cmd argv.
    let spawn_args = match spawn::finalize_spawn_args(&plan, &request.output_file) {
        Ok(args) => args,
        Err(error) => {
            sink.fail(error.to_string()).await;
            sink.shutdown().await;
            return Ok(());
        }
    };

    let log_path = spawn::log_path(&request.output_file);
    let log_file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(file) => file,
        Err(error) => {
            sink.fail(format!("could not open claude log file: {error}"))
                .await;
            sink.shutdown().await;
            return Ok(());
        }
    };

    // Typed fallible builder: Windows shim validation becomes RunState::Error
    // before spawn instead of a generic I/O failure at CreateProcess.
    let mut command = match executable.try_command(&spawn_args) {
        Ok(command) => command,
        Err(error) => {
            sink.fail(error.to_string()).await;
            sink.shutdown().await;
            return Ok(());
        }
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
            sink.fail(auth::ClaudeAuthError::BinaryMissing.to_string())
                .await;
            sink.shutdown().await;
            return Ok(());
        }
        Err(error) => {
            sink.fail(format!(
                "claude code: failed to spawn `{}`: {error}",
                executable.display()
            ))
            .await;
            sink.shutdown().await;
            return Ok(());
        }
    };

    // Prompt on stdin so shell metacharacters cannot break the command line.
    // Cancellation is selected while writing so a stuck child cannot hang us.
    if let Some(mut stdin) = child.stdin() {
        let prompt = request.prompt.clone();
        let write = async {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.shutdown().await?;
            Ok::<(), std::io::Error>(())
        };
        tokio::select! {
            biased;
            () = request.cancellation.cancelled() => {
                child.terminate().await;
                sink.stop("cancelled").await;
                sink.shutdown().await;
                return Ok(());
            }
            result = write => {
                if let Err(error) = result {
                    child.terminate().await;
                    sink.fail(format!(
                        "claude code: failed to write prompt to stdin: {error}"
                    ))
                    .await;
                    sink.shutdown().await;
                    return Ok(());
                }
            }
        }
    }

    if let Err(error) = sink.mark_running() {
        child.terminate().await;
        sink.fail(format!(
            "claude code: could not persist running status: {error}"
        ))
        .await;
        sink.shutdown().await;
        return Ok(());
    }

    let Some(stdout) = child.stdout() else {
        child.terminate().await;
        sink.fail("claude code: child stdout was not captured")
            .await;
        sink.shutdown().await;
        return Ok(());
    };

    let mut stdout = BufReader::new(stdout);
    let mut decoder = LineDecoder::default();
    let mut mapper = StreamMapper::new();
    let mut pending_terminal: Option<TerminalResult> = None;
    let mut stream_error: Option<String> = None;
    let mut chunk = vec![0_u8; 8 * 1024];

    loop {
        tokio::select! {
            biased;
            () = request.cancellation.cancelled() => {
                child.terminate().await;
                sink.stop("cancelled").await;
                sink.shutdown().await;
                return Ok(());
            }
            read = stdout.read(&mut chunk) => {
                match read {
                    Ok(0) => break,
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
                            if let Err(error) = apply_stream_line(
                                &mut mapper,
                                &mut sink,
                                &mut pending_terminal,
                                &line,
                            ) {
                                // Fatal status persistence failure ends the run.
                                stream_error = Some(error);
                                break;
                            }
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

    if stream_error.is_none() {
        match decoder.finish() {
            Ok(Some(line)) => {
                if let Err(error) =
                    apply_stream_line(&mut mapper, &mut sink, &mut pending_terminal, line)
                {
                    stream_error = Some(error);
                }
            }
            Ok(None) => {}
            Err(error) => {
                stream_error = Some(format_line_error(&error));
            }
        }
    }

    // Fatal stream decode/persistence failures terminate the tree immediately.
    // OwnedChild::terminate consumes the handle, so there is no later wait.
    if let Some(error) = stream_error {
        child.terminate().await;
        if !sink.status.state.is_terminal() {
            sink.fail(error).await;
        } else {
            sink.flush_terminal_status().await;
        }
        sink.shutdown().await;
        return Ok(());
    }

    // After stdout EOF, wait for the process while honoring cancellation. A hang
    // here would strand the full tree after the child closed stdout and slept.
    let exit_status = tokio::select! {
        biased;
        () = request.cancellation.cancelled() => {
            child.terminate().await;
            if !sink.status.state.is_terminal() {
                sink.stop("cancelled").await;
            } else {
                sink.flush_terminal_status().await;
            }
            sink.shutdown().await;
            return Ok(());
        }
        status = child.wait() => status,
    };

    let exit_status = match exit_status {
        Ok(status) => status,
        Err(error) => {
            if !sink.status.state.is_terminal() {
                sink.fail(format!("claude code: failed waiting for child: {error}"))
                    .await;
            } else {
                sink.flush_terminal_status().await;
            }
            sink.shutdown().await;
            return Ok(());
        }
    };

    // Protocol type:error is pending metadata only; final Failed/Completed is
    // chosen here after exit. Leave any already-terminal sink state alone.
    if sink.status.state.is_terminal() {
        sink.flush_terminal_status().await;
        sink.shutdown().await;
        return Ok(());
    }

    let log_tail = read_log_tail(&log_path);
    let final_outcome = decide_final_outcome(pending_terminal.as_ref(), exit_status, &log_tail);
    match final_outcome {
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

    sink.shutdown().await;
    Ok(())
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
) -> Result<(), String> {
    for effect in mapper.push_line(line) {
        if let StreamEffect::Terminal(terminal) = &effect {
            // Later terminals (for example a final `result`) replace earlier
            // pending protocol-error metadata.
            *pending_terminal = Some(terminal.clone());
        }
        sink.apply_effect(effect)?;
    }
    Ok(())
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

fn read_log_tail(path: &std::path::Path) -> String {
    let Ok(contents) = std::fs::read_to_string(path) else {
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

/// Shared helper used by the executor task after acquiring a Claude permit.
pub(crate) async fn run_bound_session(launch: ClaudeBoundLaunch) -> anyhow::Result<()> {
    run_session(ClaudeSessionRequest {
        definition: (*launch.definition).clone(),
        identity: launch.identity,
        model: launch.model,
        tools: launch.tools,
        inherit_claude_config: launch.inherit_claude_config,
        max_turns: launch.max_turns,
        prompt: launch.prompt,
        output_file: launch.output_file,
        cwd: launch.cwd,
        permission_mode: launch.permission_mode,
        cancellation: launch.cancellation,
        status_tx: Some(launch.status_tx),
        executable: None,
        auth_status: None,
        #[cfg(test)]
        persist_hooks: None,
    })
    .await
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
