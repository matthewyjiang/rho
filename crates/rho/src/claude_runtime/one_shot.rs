//! Run one no-tools `claude -p` call and return its text.
//!
//! Delegated subagents go through [`super::session`], which owns the run
//! directory, status file, and attachment contract. Rho's own internal agents
//! need none of that: they ask one question, stream the answer into a tool
//! card, and keep nothing. This module is that path. It shares auth, binary
//! resolution, argv construction, the child lifecycle, and the drain with the
//! subagent runtime, so both stay on one Claude contract.

use std::{path::PathBuf, process::Stdio};

use rho_sdk::{model::ModelUsage, CancellationToken};
use tokio::sync::watch;

use crate::agent::{OneShotPhase, OneShotUpdate, PromptPolicy};
use crate::cli_runtime::OwnedChild;

use super::{
    auth::{self, ClaudeAuthError},
    drain::{self, DrainEnd},
    executable,
    line_decoder::MAX_NDJSON_LINE_BYTES,
    spawn::{self, ClaudePermissionMode, ClaudeSpawnRequest, SessionPersistence},
    stream::{StreamEffect, StreamMapper, TerminalResult},
    terminal::{assess_terminal, TerminalOutcome},
};

pub(crate) const CANCELLATION_ERROR: &str = "claude code: cancelled";

/// Claude CLI permission mode for no-tools one-shots (advisor).
///
/// One-shots set Claude `dontAsk` directly so they stay independent of host
/// permission mode. They expose no tools, so Claude's extra dontAsk approvals
/// (read-only Bash, PreToolUse hooks) have nothing to run. Delegated Auto and
/// Allow edits map to `dontAsk` only when
/// [`super::spawn::map_permission_mode`] can keep the child on the bound set.
/// [`ClaudePermissionMode::Plan`] injects AskUserQuestion / ExitPlanMode text
/// and poisons advisor prose even when tools is empty.
pub(crate) const ONE_SHOT_PERMISSION_MODE: ClaudePermissionMode = ClaudePermissionMode::DontAsk;

/// A single Claude question with no tools and no follow-up turn.
pub(crate) struct ClaudeOneShotRequest {
    /// One of Rho's own constant prompts. It travels on argv, which other
    /// processes can read, so it must never carry user or workspace text.
    pub(crate) system_prompt: &'static str,
    /// The user turn, written to the child's stdin.
    pub(crate) input: String,
    /// Pass-through `--model`. `None` omits the flag.
    pub(crate) model: Option<String>,
    /// Bound reasoning. `None` omits `--effort`.
    pub(crate) reasoning: Option<rho_providers::reasoning::ReasoningLevel>,
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

    let plan = spawn::build_spawn_plan(&one_shot_spawn_request(&request));

    let mut command = executable
        .try_command(spawn::inline_prompt_args(&plan))
        .map_err(|error| format!("claude code: {error}"))?;
    command
        .current_dir(&plan.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // No run directory means no log file, so stderr comes back on a pipe
        // and the drain keeps a bounded tail of it for failure text.
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = OwnedChild::spawn(command).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ClaudeAuthError::BinaryMissing.to_string()
        } else {
            format!(
                "claude code: failed to spawn `{}`: {error}",
                executable.display()
            )
        }
    })?;

    let mut text = String::new();
    let drained = {
        let mut mapper = StreamMapper::new();
        let mut on_effect = |effect| apply_effect(effect, &mut text, &mut stream);
        drain::drain_child(
            &mut child,
            drain::DrainConfig {
                program_label: "claude code",
                max_line_bytes: MAX_NDJSON_LINE_BYTES,
            },
            &mut mapper,
            drain::DrainInput::Text {
                prompt: request.input.clone(),
            },
            &request.cancellation,
            &mut on_effect,
        )
        .await
    };
    // Only a reaped exit guarantees the tree is gone.
    if !matches!(drained.end, DrainEnd::Exited(Ok(_))) {
        child.terminate().await;
    }

    match drained.end {
        DrainEnd::Cancelled => Err(CANCELLATION_ERROR.into()),
        DrainEnd::StdinFailed(error) | DrainEnd::StreamFailed(error) => Err(error),
        DrainEnd::Exited(Err(error)) => {
            Err(format!("claude code: failed waiting for child: {error}"))
        }
        DrainEnd::Exited(Ok(status)) => finish(text, drained.terminal, &drained.stderr, status),
    }
}

/// Spawn contract used by [`run_one_shot`]. Kept as one helper so regression
/// tests assert the same request shape production builds.
fn one_shot_spawn_request(request: &ClaudeOneShotRequest) -> ClaudeSpawnRequest {
    ClaudeSpawnRequest {
        system_prompt: PromptPolicy::Replace(request.system_prompt.to_string()),
        model: request.model.clone(),
        // Parity with the Rho one-shot path, which exposes no tools at all.
        tools: Vec::new(),
        inherit_claude_config: false,
        // Claude-native mode, not Rho PermissionMode. See ONE_SHOT_PERMISSION_MODE.
        permission_mode: ONE_SHOT_PERMISSION_MODE,
        cwd: request.cwd.clone(),
        max_turns: 1,
        reasoning: request.reasoning,
        session_persistence: SessionPersistence::Discard,
        // One-shot has no parent messaging path; keep plain-text stdin.
        input_format: spawn::ClaudeInputFormat::Text,
    }
}

/// Builds the advisor result after the shared Claude terminal assessment.
fn finish(
    text: String,
    terminal: Option<TerminalResult>,
    stderr: &str,
    status: std::process::ExitStatus,
) -> Result<ClaudeOneShotResult, String> {
    if !status.success() && spawn::looks_like_max_turns_unsupported(stderr) {
        return Err(
            "claude code: this claude binary rejected --max-turns; upgrade Claude Code or remove the turn cap"
                .into(),
        );
    }
    let terminal = match assess_terminal(terminal, status, stderr, "claude code") {
        TerminalOutcome::Success(terminal) => terminal,
        TerminalOutcome::Failure { detail, .. } => return Err(detail),
    };

    // Prefer the terminal `result` text: it is the complete answer, while the
    // streamed deltas are bounded for display.
    let answer = terminal
        .result_text
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(text);
    let mut usage = terminal.usage.unwrap_or_default();
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

/// Accumulate the answer and keep the advisor card current.
fn apply_effect(effect: StreamEffect, text: &mut String, stream: &mut OneShotStream) {
    match effect {
        StreamEffect::Status(patch) => {
            if let Some(appended) = patch.append_text {
                text.push_str(&appended);
                stream.publish_text(OneShotPhase::Responding, text);
            } else if patch.last_activity.as_deref() == Some("reasoning") {
                stream.publish(OneShotPhase::Thinking);
            }
        }
        // The drain records terminal results; attachments and rate-limit
        // notices belong to the subagent contract, and a one-shot call has no
        // run artifacts to write.
        StreamEffect::Terminal(_) | StreamEffect::Attachment(_) | StreamEffect::RateLimit(_) => {}
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

#[cfg(test)]
#[path = "one_shot_tests.rs"]
mod tests;
