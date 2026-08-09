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

use crate::{
    agent::{OneShotPhase, OneShotUpdate, PromptPolicy},
    permission::PermissionMode,
};

use super::{
    auth::{self, ClaudeAuthError},
    child::OwnedChild,
    drain::{self, DrainEnd},
    executable,
    spawn::{self, ClaudeSpawnRequest, SessionPersistence},
    stream::{StreamEffect, TerminalClassification, TerminalResult},
};

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
        let mut on_effect = |effect| apply_effect(effect, &mut text, &mut stream);
        drain::drain_child(
            &mut child,
            &request.input,
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
        DrainEnd::Cancelled => Err("the advisor request was cancelled".into()),
        DrainEnd::StdinFailed(error) | DrainEnd::StreamFailed(error) => Err(error),
        DrainEnd::Exited(Err(error)) => {
            Err(format!("claude code: failed waiting for child: {error}"))
        }
        DrainEnd::Exited(Ok(status)) => finish(text, drained.terminal, &drained.stderr, status),
    }
}

/// Combines the terminal message with the exit status, the same rule the
/// subagent runtime applies: only an explicit success plus a clean exit counts.
fn finish(
    text: String,
    terminal: Option<TerminalResult>,
    stderr: &str,
    status: std::process::ExitStatus,
) -> Result<ClaudeOneShotResult, String> {
    let exit_ok = status.success();
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
