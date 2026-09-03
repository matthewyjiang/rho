//! External CLI session adapter over the shared run-artifact sink.
//!
//! Shared by Claude Code and Cursor. Labels and stream mappers differ per CLI.
//!
//! Translates stream-json effects into the generic status/attachment contract.
//! Rate-limit cache updates are collected here and flushed once at settle so
//! the artifact path never knows about `/limits`.

use std::path::PathBuf;

use tokio::sync::watch;

use crate::{
    run_artifacts::{AttachmentEvent, LiveRunTitle, RunArtifactIdentity, RunArtifactSink},
    subagent::RunStatus,
};

use super::super::{
    rate_limit::{self, RateLimitObservation, RateLimitState},
    stream::{self, apply_status_patch, StreamEffect, TerminalResult},
};

/// Starting activity, program name, and model-facing copy for a CLI runtime sink.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RuntimeLabel {
    pub(crate) starting_activity: &'static str,
    pub(crate) program: &'static str,
    /// Binary invoked for `--resume <session-id>`.
    pub(crate) resume_command: &'static str,
    /// Prefix for session metadata, e.g. `claude session`.
    pub(crate) session_label: &'static str,
    /// Cost line label, e.g. `claude cost`.
    pub(crate) cost_label: &'static str,
}

/// Claude Code labels for [`StatusSink`].
pub(crate) const CLAUDE_LABEL: RuntimeLabel = RuntimeLabel {
    starting_activity: "starting claude",
    program: "claude code",
    resume_command: "claude",
    session_label: "claude session",
    cost_label: "claude cost",
};

/// Thin Claude-facing handle around [`RunArtifactSink`].
pub(crate) struct StatusSink {
    inner: RunArtifactSink,
    pending_limits: RateLimitState,
    /// Override for tests. Production leaves this unset and uses
    /// [`rate_limit::default_state_path`] at flush time.
    rate_limit_state_path: Option<PathBuf>,
}

impl StatusSink {
    pub(crate) fn new(
        path: PathBuf,
        identity: &RunArtifactIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        rate_limit_state_path: Option<PathBuf>,
        label: RuntimeLabel,
    ) -> anyhow::Result<Self> {
        let mut inner = RunArtifactSink::open(path, identity, prompt, status_tx)?;
        inner.status.last_activity = Some(label.starting_activity.into());
        inner.publish();
        Ok(Self {
            inner,
            pending_limits: RateLimitState::default(),
            rate_limit_state_path,
        })
    }

    /// Resume after the executor already wrote the Starting boundary.
    pub(crate) fn continue_from(
        path: PathBuf,
        mut status: RunStatus,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        live_title: Option<LiveRunTitle>,
        rate_limit_state_path: Option<PathBuf>,
        label: RuntimeLabel,
    ) -> anyhow::Result<Self> {
        status.last_activity = Some(label.starting_activity.into());
        let mut inner =
            RunArtifactSink::continue_from(path, status, prompt, status_tx, live_title)?;
        inner.publish();
        Ok(Self {
            inner,
            pending_limits: RateLimitState::default(),
            rate_limit_state_path,
        })
    }

    pub(crate) fn status(&self) -> &RunStatus {
        &self.inner.status
    }

    pub(crate) fn mark_running(&mut self) {
        self.inner.mark_running("running");
    }

    pub(crate) fn apply_effect(&mut self, effect: StreamEffect) {
        match effect {
            StreamEffect::Attachment(event) => {
                // The Claude path deliberately mirrors reasoning into
                // `last_text` as well as answer text, unlike the Rho reporter,
                // which keeps the thinking out of the status file.
                if let AttachmentEvent::AssistantTextDelta(text)
                | AttachmentEvent::ReasoningDelta(text) = &event
                {
                    if !text.is_empty() {
                        self.inner.append_last_text(text);
                    }
                }
                self.inner.record_attachment(event);
            }
            StreamEffect::Status(patch) => {
                apply_status_patch(&mut self.inner.status, patch);
                self.inner.publish();
            }
            StreamEffect::RateLimit(info) => {
                self.inner.write_attachment(AttachmentEvent::Notice(format!(
                    "claude limits: {}",
                    stream::describe_rate_limit(&info)
                )));
                self.pending_limits
                    .merge_window(RateLimitObservation::capture(info));
                self.inner.publish();
            }
            // Terminal payloads are pending metadata until process exit.
            StreamEffect::Terminal(terminal) => {
                apply_terminal_metadata(&mut self.inner.status, &terminal);
                self.inner.publish();
            }
        }
    }

    pub(crate) async fn fail(&mut self, error: impl Into<String>) {
        self.inner.finish_error(error);
        self.flush_rate_limits().await;
    }

    pub(crate) async fn stop(&mut self, reason: &str, pending: Option<&TerminalResult>) {
        if let Some(terminal) = pending {
            apply_terminal_metadata(&mut self.inner.status, terminal);
        }
        self.inner.finish_stopped(reason);
        self.flush_rate_limits().await;
    }

    pub(crate) async fn finalize_success_from_stream(&mut self, terminal: &TerminalResult) {
        apply_terminal_metadata(&mut self.inner.status, terminal);
        self.inner.finish_ok(terminal.result_text.clone());
        self.flush_rate_limits().await;
    }

    pub(crate) async fn finalize_failure_from_stream(
        &mut self,
        terminal: Option<&TerminalResult>,
        detail: String,
        prefer_detail: bool,
    ) {
        if let Some(terminal) = terminal {
            apply_terminal_metadata(&mut self.inner.status, terminal);
        }
        let error = if prefer_detail {
            detail
        } else {
            terminal
                .and_then(|terminal| {
                    terminal
                        .error
                        .clone()
                        .filter(|text| !text.is_empty())
                        .or_else(|| terminal.result_text.clone())
                })
                .unwrap_or(detail)
        };
        self.inner.finish_error(error);
        self.flush_rate_limits().await;
    }

    async fn flush_rate_limits(&mut self) {
        let pending = std::mem::take(&mut self.pending_limits);
        if pending.is_empty() {
            return;
        }
        let path = match self.rate_limit_state_path.clone() {
            Some(path) => path,
            None => match rate_limit::default_state_path() {
                Ok(path) => path,
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        "claude rate-limit cache path unavailable; dropping pending windows"
                    );
                    return;
                }
            },
        };
        // File lock + atomic write are blocking; keep them off the runtime worker.
        let result =
            tokio::task::spawn_blocking(move || rate_limit::store_state(&path, pending)).await;
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "failed to persist claude rate-limit cache");
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "claude rate-limit cache flush task failed to join"
                );
            }
        }
    }
}

fn apply_terminal_metadata(status: &mut RunStatus, terminal: &TerminalResult) {
    if let Some(session_id) = terminal.session_id.clone() {
        status.claude_session_id = Some(session_id);
    }
    if let Some(turns) = terminal.num_turns {
        status.turns = turns;
    }
    if let Some(usage) = &terminal.usage {
        if let Some(tokens) = usage.inclusive_prompt_tokens() {
            status.input_tokens = Some(tokens);
        }
        if let Some(tokens) = usage.output_tokens {
            status.output_tokens = Some(tokens);
        }
    }
    if let Some(cost) = terminal.total_cost_usd {
        status.total_cost_usd = Some(cost);
    }
    if let Some(result) = terminal.result_text.clone() {
        status.result = Some(result);
    }
    if let Some(error) = terminal.error.clone() {
        status.error = Some(error);
    }
}
