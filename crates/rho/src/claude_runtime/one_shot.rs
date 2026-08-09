//! Run one no-tools `claude -p` call and return its text.
//!
//! Delegated subagents go through [`super::session`], which owns the run
//! directory, status file, and attachment contract. Rho's own internal agents
//! need none of that: they ask one question, stream the answer into a tool
//! card, and keep nothing. This module is that path. It shares auth, binary
//! resolution, argv construction, and the stream mapper with the subagent
//! runtime, so both stay on one Claude contract.

use std::{path::PathBuf, process::Stdio, time::Duration};

use rho_sdk::{model::ModelUsage, CancellationToken};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    sync::watch,
};

use crate::{
    agent::{OneShotPhase, OneShotUpdate, PromptPolicy},
    permission::PermissionMode,
    tools::process::{prepare_child_command, ProcessTree},
};

use super::{
    auth::{self, ClaudeAuthError},
    executable,
    line_decoder::claude_ndjson_line_decoder,
    spawn::{self, ClaudeSpawnRequest, SessionPersistence},
    stream::{StreamEffect, StreamMapper, TerminalClassification, TerminalResult},
};

/// Bytes of child stderr kept for diagnosis. The one-shot path writes no log
/// file, so a failure has to explain itself from memory.
const MAX_STDERR_BYTES: usize = 8 * 1024;

/// A single Claude question with no tools and no follow-up turn.
pub(crate) struct ClaudeOneShotRequest {
    /// One of Rho's own constant prompts. It travels on argv, which other
    /// processes can read, so it must never carry user or workspace text.
    pub(crate) system_prompt: &'static str,
    /// The user turn, written to the child's stdin.
    pub(crate) input: String,
    /// Pass-through `--model`. `None` omits the flag.
    pub(crate) model: Option<String>,
    /// Pass-through `--effort`. `None` omits the flag.
    pub(crate) effort: Option<&'static str>,
    pub(crate) cwd: PathBuf,
    pub(crate) cancellation: CancellationToken,
}

/// Text and usage from a finished one-shot Claude call.
pub(crate) struct ClaudeOneShotResult {
    pub(crate) text: String,
    pub(crate) usage: ModelUsage,
}

/// Runs the request to completion.
///
/// Every failure is user-facing text, because callers surface it as a tool
/// error rather than failing the parent turn. Assistant text streams into
/// `updates` as it arrives; reasoning never does, only the phase moves to
/// [`OneShotPhase::Thinking`].
pub(crate) async fn run_one_shot(
    request: ClaudeOneShotRequest,
    updates: Option<watch::Sender<OneShotUpdate>>,
) -> Result<ClaudeOneShotResult, String> {
    let mut stream = OneShotStream::new(updates);
    stream.publish(OneShotPhase::WaitingForProvider);

    match auth::query().await {
        Ok(status) if status.logged_in => {}
        Ok(_) => return Err("claude code: not signed in - run /login claude-code".into()),
        Err(ClaudeAuthError::BinaryMissing) => {
            return Err(ClaudeAuthError::BinaryMissing.to_string())
        }
        Err(error) => return Err(format!("claude code: auth preflight failed: {error}")),
    }
    let executable = executable::resolve().map_err(|error| error.to_string())?;

    let plan = spawn::build_spawn_plan(&ClaudeSpawnRequest {
        system_prompt: PromptPolicy::Replace(request.system_prompt.to_string()),
        model: request.model.clone(),
        // Parity with the Rho one-shot path, which exposes no tools at all.
        tools: Vec::new(),
        inherit_claude_config: false,
        // Fixed, not the session's mode: with no tools there is nothing to
        // approve, and Supervised would otherwise refuse the spawn outright.
        permission_mode: PermissionMode::Plan,
        cwd: request.cwd.clone(),
        max_turns: 1,
        effort: request.effort,
        session_persistence: SessionPersistence::Discard,
    })
    .map_err(|error| error.to_string())?;

    let mut command = executable
        .try_command(spawn::inline_prompt_args(&plan))
        .map_err(|error| error.to_string())?;
    command
        .current_dir(&plan.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    prepare_child_command(&mut command);

    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ClaudeAuthError::BinaryMissing.to_string()
        } else {
            format!(
                "claude code: failed to spawn `{}`: {error}",
                executable.display()
            )
        }
    })?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.start_kill();
            return Err(format!("claude code: could not track the child: {error}"));
        }
    };
    let mut child = OwnedChild { child, tree };

    let outcome = drain(&request, &mut child, &mut stream).await;
    // Only a reaped exit guarantees the tree is gone.
    if !matches!(outcome, Ok(Drained { exited: true, .. })) {
        child.terminate().await;
    }
    let drained = outcome?;
    finish(drained)
}

/// What one drained child produced before its exit status was judged.
struct Drained {
    text: String,
    terminal: Option<TerminalResult>,
    stderr: String,
    exited: bool,
    exit_ok: bool,
}

async fn drain(
    request: &ClaudeOneShotRequest,
    child: &mut OwnedChild,
    stream: &mut OneShotStream,
) -> Result<Drained, String> {
    let stdout = child
        .child
        .stdout
        .take()
        .ok_or_else(|| "claude code: child stdout was not captured".to_string())?;
    let stderr = child.child.stderr.take();
    let stdin = child.child.stdin.take();

    // Write stdin concurrently with the stdout drain: a child that emits enough
    // output before reading its prompt would otherwise fill the pipe and hang.
    let prompt = request.input.clone();
    let stdin_write = async move {
        let Some(mut stdin) = stdin else {
            return Ok(());
        };
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await
    };
    let read_stderr = async move {
        let Some(mut stderr) = stderr else {
            return String::new();
        };
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer).await;
        buffer.truncate(MAX_STDERR_BYTES);
        String::from_utf8_lossy(&buffer).trim().to_string()
    };
    tokio::pin!(stdin_write);
    tokio::pin!(read_stderr);

    let mut stdout = BufReader::new(stdout);
    let mut decoder = claude_ndjson_line_decoder();
    let mut mapper = StreamMapper::new();
    let mut text = String::new();
    let mut terminal: Option<TerminalResult> = None;
    let mut stderr_text = String::new();
    let mut stdin_error: Option<String> = None;
    let mut stream_error: Option<String> = None;
    let mut stdin_done = false;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut chunk = vec![0_u8; 8 * 1024];

    while !(stdin_done && stdout_done && stderr_done) {
        tokio::select! {
            biased;
            () = request.cancellation.cancelled() => {
                return Err("the advisor request was cancelled".into());
            }
            result = &mut stdin_write, if !stdin_done => {
                stdin_done = true;
                if let Err(error) = result {
                    stdin_error =
                        Some(format!("claude code: failed to write the prompt to stdin: {error}"));
                    break;
                }
            }
            captured = &mut read_stderr, if !stderr_done => {
                stderr_done = true;
                stderr_text = captured;
            }
            read = stdout.read(&mut chunk), if !stdout_done => {
                match read {
                    Ok(0) => stdout_done = true,
                    Ok(count) => {
                        decoder.push(&chunk[..count]);
                        loop {
                            match decoder.next_line() {
                                Ok(Some(line)) => {
                                    let line = line.to_string();
                                    apply_line(&mut mapper, &line, &mut text, &mut terminal, stream);
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    stream_error = Some(format!("claude code: {error}"));
                                    break;
                                }
                            }
                        }
                        if stream_error.is_some() {
                            break;
                        }
                    }
                    Err(error) => {
                        stream_error =
                            Some(format!("claude code: failed reading stdout: {error}"));
                        break;
                    }
                }
            }
        }
    }

    if let Some(error) = stdin_error {
        return Err(error);
    }
    if stream_error.is_none() {
        match decoder.finish() {
            Ok(Some(line)) => apply_line(&mut mapper, line, &mut text, &mut terminal, stream),
            Ok(None) => {}
            Err(error) => stream_error = Some(format!("claude code: {error}")),
        }
    }
    if let Some(error) = stream_error {
        return Err(error);
    }

    let status = tokio::select! {
        biased;
        () = request.cancellation.cancelled() => {
            return Err("the advisor request was cancelled".into());
        }
        status = child.wait() => status,
    };
    let status =
        status.map_err(|error| format!("claude code: failed waiting for child: {error}"))?;
    Ok(Drained {
        text,
        terminal,
        stderr: stderr_text,
        exited: true,
        exit_ok: status.success(),
    })
}

/// Combines the terminal message with the exit status, the same rule the
/// subagent runtime applies: only an explicit success plus a clean exit counts.
fn finish(drained: Drained) -> Result<ClaudeOneShotResult, String> {
    let Drained {
        text,
        terminal,
        stderr,
        exit_ok,
        ..
    } = drained;
    let detail = |fallback: &str| {
        if stderr.is_empty() {
            fallback.to_string()
        } else {
            format!("{fallback}: {stderr}")
        }
    };
    let Some(terminal) = terminal else {
        return Err(detail("claude code: the run ended with no result message"));
    };
    match &terminal.classification {
        TerminalClassification::Success { .. } if exit_ok => {}
        TerminalClassification::Success { .. } => {
            return Err(detail(
                "claude code: the run reported success but exited nonzero",
            ));
        }
        TerminalClassification::Failure { subtype, .. } => {
            return Err(detail(&format!("claude code: run failed ({subtype})")));
        }
        TerminalClassification::Invalid { reason } => {
            return Err(detail(&format!("claude code: {reason}")));
        }
    }

    // Prefer the terminal `result` text: it is the complete answer, while the
    // streamed deltas are bounded for display.
    let answer = terminal
        .result_text
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(text);
    let mut usage = terminal.usage.clone().unwrap_or_default();
    // Claude reports subscription spend as dollars on the result message; the
    // usage ledger and the parent session total both count micros.
    if let Some(cost) = terminal.total_cost_usd.filter(|cost| *cost > 0.0) {
        usage.cost_usd_micros = Some((cost * 1_000_000.0).round() as u64);
    }
    Ok(ClaudeOneShotResult {
        text: answer.trim().to_string(),
        usage,
    })
}

fn apply_line(
    mapper: &mut StreamMapper,
    line: &str,
    text: &mut String,
    terminal: &mut Option<TerminalResult>,
    stream: &mut OneShotStream,
) {
    for effect in mapper.push_line(line) {
        match effect {
            StreamEffect::Status(patch) => {
                if let Some(appended) = patch.append_text {
                    text.push_str(&appended);
                    stream.publish_text(OneShotPhase::Responding, text);
                } else if patch.last_activity.as_deref() == Some("reasoning") {
                    stream.publish(OneShotPhase::Thinking);
                }
            }
            StreamEffect::Terminal(result) => *terminal = Some(result),
            // Attachments and rate-limit notices belong to the subagent
            // contract; a one-shot call has no run artifacts to write.
            StreamEffect::Attachment(_) | StreamEffect::RateLimit(_) => {}
        }
    }
}

/// Latest-wins publisher for the advisor card, mirroring the Rho one-shot path.
struct OneShotStream {
    updates: Option<watch::Sender<OneShotUpdate>>,
    text: String,
}

impl OneShotStream {
    fn new(updates: Option<watch::Sender<OneShotUpdate>>) -> Self {
        Self {
            updates,
            text: String::new(),
        }
    }

    fn publish(&mut self, phase: OneShotPhase) {
        let text = self.text.clone();
        self.send(phase, &text);
    }

    fn publish_text(&mut self, phase: OneShotPhase, text: &str) {
        self.text = text.to_string();
        self.send(phase, text);
    }

    fn send(&self, phase: OneShotPhase, text: &str) {
        if let Some(updates) = &self.updates {
            let _ = updates.send(OneShotUpdate::new(phase, text));
        }
    }
}

/// A spawned child plus its process group, so no path leaves a live tree.
struct OwnedChild {
    child: tokio::process::Child,
    tree: ProcessTree,
}

impl OwnedChild {
    async fn terminate(&mut self) {
        self.tree
            .terminate(&mut self.child, Duration::from_millis(200))
            .await;
    }

    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait().await;
        self.tree.kill();
        status
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.tree.kill();
        let _ = self.child.start_kill();
    }
}
