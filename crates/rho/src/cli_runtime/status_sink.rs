//! External CLI session adapter over the shared run-artifact sink.
//!
//! Shared by Claude Code and Cursor. Labels and stream mappers differ per CLI.
//!
//! Translates stream-json effects into the generic status/attachment contract.
//! Rate-limit persistence is a pluggable [`RateLimitRecorder`]: Claude records
//! subscription windows; Cursor leaves the recorder unset.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use tokio::sync::watch;

use crate::{
    run_artifacts::{AttachmentEvent, LiveRunTitle, RunArtifactIdentity, RunArtifactSink},
    subagent::RunStatus,
};

use super::stream_effect::{RateLimitInfo, StreamEffect, TerminalResult};
use super::stream_format::apply_status_patch;

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

/// CLI-specific rate-limit cache updates collected during a run.
///
/// Object-safe so the sink can hold an optional recorder without knowing the
/// owning runtime. `flush` is `'static` after taking pending state.
pub(crate) trait RateLimitRecorder: Send {
    /// Optional transcript notice for this observation.
    fn notice(&self, info: &RateLimitInfo) -> Option<String>;
    fn record(&mut self, info: RateLimitInfo);
    fn flush(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// Thin CLI-facing handle around [`RunArtifactSink`].
pub(crate) struct StatusSink {
    inner: RunArtifactSink,
    rate_limits: Option<Box<dyn RateLimitRecorder>>,
}

impl StatusSink {
    pub(crate) fn new(
        path: PathBuf,
        identity: &RunArtifactIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        rate_limits: Option<Box<dyn RateLimitRecorder>>,
        label: RuntimeLabel,
    ) -> anyhow::Result<Self> {
        let mut inner = RunArtifactSink::open(path, identity, prompt, status_tx)?;
        inner.status.last_activity = Some(label.starting_activity.into());
        inner.publish();
        Ok(Self { inner, rate_limits })
    }

    /// Resume after the executor already wrote the Starting boundary.
    pub(crate) fn continue_from(
        path: PathBuf,
        mut status: RunStatus,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        live_title: Option<LiveRunTitle>,
        rate_limits: Option<Box<dyn RateLimitRecorder>>,
        label: RuntimeLabel,
    ) -> anyhow::Result<Self> {
        status.last_activity = Some(label.starting_activity.into());
        let mut inner =
            RunArtifactSink::continue_from(path, status, prompt, status_tx, live_title)?;
        inner.publish();
        Ok(Self { inner, rate_limits })
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
                // CLI runtimes mirror reasoning into `last_text` as well as
                // answer text, unlike the Rho reporter, which keeps thinking
                // out of the status file.
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
                if let Some(recorder) = self.rate_limits.as_mut() {
                    if let Some(notice) = recorder.notice(&info) {
                        self.inner.write_attachment(AttachmentEvent::Notice(notice));
                    }
                    recorder.record(info);
                    self.inner.publish();
                }
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
        if let Some(recorder) = self.rate_limits.as_mut() {
            recorder.flush().await;
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

#[cfg(test)]
#[path = "status_sink_tests.rs"]
mod tests;
