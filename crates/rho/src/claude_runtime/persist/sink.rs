//! StatusSink orchestration for one Claude CLI run.

use std::{
    sync::{mpsc as std_mpsc, Arc, Mutex},
    thread::JoinHandle,
};

use tokio::sync::{oneshot, watch};

use crate::{
    subagent::{self, RunState, RunStatus},
    tui::{AttachmentEvent, AttachmentWriter},
};

use super::super::{
    rate_limit::{RateLimitObservation, RateLimitSlot},
    stream::{self, StreamEffect, TerminalResult},
};
#[cfg(test)]
use super::worker::PersistHooks;
use super::worker::{
    demote_completed_attachment, demote_status_for_sticky_error, spawn_persist_worker, BarrierAck,
    ClaudeRunIdentity, PersistCommand, PersistFeedback,
};

const MAX_SINK_TEXT_BYTES: usize = 256 * 1024;

/// In-memory + durable sink for one Claude CLI run.
pub(crate) struct StatusSink {
    path: std::path::PathBuf,
    pub(crate) status: RunStatus,
    status_tx: Option<watch::Sender<RunStatus>>,
    persist_tx: Option<std_mpsc::SyncSender<PersistCommand>>,
    persist_join: Option<JoinHandle<()>>,
    feedback: Arc<Mutex<PersistFeedback>>,
    rate_limit_slot: Arc<RateLimitSlot>,
    attachment_enabled: bool,
    first_status_error: Option<String>,
    closed: bool,
}

impl StatusSink {
    pub(crate) fn new(
        path: std::path::PathBuf,
        identity: &ClaudeRunIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
    ) -> anyhow::Result<Self> {
        #[cfg(test)]
        {
            Self::build(path, identity, prompt, status_tx, PersistHooks::default())
        }
        #[cfg(not(test))]
        {
            Self::build(path, identity, prompt, status_tx)
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_hooks(
        path: std::path::PathBuf,
        identity: &ClaudeRunIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        hooks: PersistHooks,
    ) -> anyhow::Result<Self> {
        Self::build(path, identity, prompt, status_tx, hooks)
    }

    fn build(
        path: std::path::PathBuf,
        identity: &ClaudeRunIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        #[cfg(test)] hooks: PersistHooks,
    ) -> anyhow::Result<Self> {
        let model = identity
            .model
            .clone()
            .unwrap_or_else(|| "claude-cli".into());
        let mut status = RunStatus {
            state: RunState::Starting,
            agent_id: Some(identity.agent_id.clone()),
            agent_fingerprint: Some(identity.agent_fingerprint.clone()),
            provider: Some("claude-code".into()),
            model: Some(model),
            last_activity: Some("starting claude".into()),
            ..RunStatus::default()
        };

        // Opening the attachment journal is local startup work. Stream drain
        // later stays free of sync file I/O on the async task.
        let (attachment, attachment_error) = match AttachmentWriter::create(&path) {
            Ok(mut writer) => {
                match writer.write_event(&AttachmentEvent::Prompt(prompt.to_string())) {
                    Ok(()) => (Some(writer), None),
                    Err(error) => (
                        None,
                        Some(format!("could not record attach output: {error}")),
                    ),
                }
            }
            Err(error) => (
                None,
                Some(format!("could not record attach output: {error}")),
            ),
        };
        status.attachment_error = attachment_error;
        let attachment_enabled = status.attachment_error.is_none();
        // Run boundary: deliberately replace any prior terminal result.json so
        // attach sees Starting promptly even when reusing an output path.
        subagent::initialize_status(&path, &status)?;
        if let Some(tx) = &status_tx {
            tx.send_replace(status.clone());
        }

        let rate_limit_slot = Arc::new(RateLimitSlot::new());
        let (persist_tx, feedback, persist_join) = spawn_persist_worker(
            path.clone(),
            attachment,
            Some(status.clone()),
            Arc::clone(&rate_limit_slot),
            #[cfg(test)]
            hooks,
        );

        Ok(Self {
            path,
            status,
            status_tx,
            persist_tx: Some(persist_tx),
            persist_join: Some(persist_join),
            feedback,
            rate_limit_slot,
            attachment_enabled,
            first_status_error: None,
            closed: false,
        })
    }

    fn publish_watch(&self) {
        if let Some(tx) = &self.status_tx {
            tx.send_replace(self.status.clone());
        }
    }

    fn take_attachment_feedback(&mut self) {
        let error = self
            .feedback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .attachment_error
            .take();
        if let Some(error) = error {
            self.attachment_enabled = false;
            if self.status.attachment_error.is_none() {
                self.status.attachment_error = Some(error);
            }
        }
    }

    fn note_send_failure(&mut self, kind: &str) {
        let message = format!("{kind} persistence worker stopped");
        if self.first_status_error.is_none() {
            self.first_status_error = Some(message);
        }
    }

    /// Enqueue status without blocking. Nonterminal watch updates stay live;
    /// terminal snapshots wait for [`Self::finish_with_barrier`].
    fn queue_status(&mut self, force: bool) -> Result<(), String> {
        if !self.status.state.is_terminal() {
            self.publish_watch();
        }
        self.take_attachment_feedback();
        if self.closed {
            return self.status_error_result();
        }
        let Some(tx) = self.persist_tx.as_ref() else {
            return self.status_error_result();
        };
        let command = PersistCommand::Status {
            status: self.status.clone(),
            force,
        };
        match tx.try_send(command) {
            Ok(()) => {}
            Err(std_mpsc::TrySendError::Full(_)) => {
                // Keep attachment capacity. Replaceable status is dropped; a
                // later force/barrier still publishes the latest snapshot.
            }
            Err(std_mpsc::TrySendError::Disconnected(_)) => {
                self.note_send_failure("status");
                return self.status_error_result();
            }
        }
        self.pull_status_error_feedback();
        self.status_error_result()
    }

    fn pull_status_error_feedback(&mut self) {
        let error = self
            .feedback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .first_status_error
            .clone();
        if let Some(error) = error {
            if self.first_status_error.is_none() {
                self.first_status_error = Some(error);
            }
        }
    }

    fn status_error_result(&self) -> Result<(), String> {
        match &self.first_status_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn queue_attachment(&mut self, event: AttachmentEvent) {
        if !self.attachment_enabled || self.closed {
            return;
        }
        let Some(tx) = self.persist_tx.as_ref() else {
            self.disable_attachment("persistence worker stopped");
            return;
        };
        match tx.try_send(PersistCommand::Attachment(event)) {
            Ok(()) => {
                self.take_attachment_feedback();
            }
            Err(std_mpsc::TrySendError::Full(_)) => {
                // Never silently drop transcript events: degrade recording.
                self.disable_attachment("recording could not keep up");
                let _ = self.queue_status(/*force*/ false);
            }
            Err(std_mpsc::TrySendError::Disconnected(_)) => {
                self.disable_attachment("persistence worker stopped");
                let _ = self.queue_status(/*force*/ false);
            }
        }
    }

    fn disable_attachment(&mut self, reason: &str) {
        self.attachment_enabled = false;
        if self.status.attachment_error.is_none() {
            self.status.attachment_error =
                Some(format!("could not record attach output: {reason}"));
        }
    }

    fn queue_rate_limit(&mut self, info: stream::RateLimitInfo) {
        if self.closed {
            return;
        }
        // Capture order at the stream effect boundary, outside the bounded
        // transcript queue, so saturation cannot drop the observation.
        let observation = RateLimitObservation::capture(info);
        self.rate_limit_slot.publish(observation);
        let Some(tx) = self.persist_tx.as_ref() else {
            return;
        };
        // Wake the worker when possible. If the queue is full the slot still
        // holds the latest value for the next wake or terminal barrier.
        let _ = tx.try_send(PersistCommand::FlushRateLimit);
    }

    pub(crate) fn apply_effect(&mut self, effect: StreamEffect) -> Result<(), String> {
        match effect {
            StreamEffect::Attachment(event) => {
                self.queue_attachment(event);
                Ok(())
            }
            StreamEffect::Status(patch) => {
                // Protocol type:error and result messages only carry metadata
                // here; final Ok/Error is decided after child wait.
                stream::apply_status_patch(&mut self.status, patch);
                let force = self.status.state.is_terminal();
                self.queue_status(force)
            }
            StreamEffect::RateLimit(info) => {
                self.queue_rate_limit(info);
                Ok(())
            }
            StreamEffect::Terminal(terminal) => {
                apply_terminal_metadata(&mut self.status, &terminal);
                // Keep pending; final Ok/Error is decided after process exit.
                self.queue_status(/*force*/ false)
            }
        }
    }

    pub(crate) async fn fail(&mut self, error: impl Into<String>) {
        if self.status.state.is_terminal() {
            return;
        }
        let error = bound_text(error.into());
        self.status.state = RunState::Error;
        self.status.error = Some(error.clone());
        self.status.last_activity = Some("failed".into());
        let _ = self
            .finish_with_barrier(Some(AttachmentEvent::Failed(error)))
            .await;
    }

    pub(crate) async fn stop(&mut self, reason: &str) {
        if self.status.state.is_terminal() {
            return;
        }
        self.status.state = RunState::Stopped;
        self.status.last_activity = Some(reason.into());
        if self.status.result.is_none() {
            self.status.result = self
                .status
                .last_text
                .as_ref()
                .map(|text| format!("(partial, stopped before finishing)\n{text}"));
        }
        let _ = self
            .finish_with_barrier(Some(AttachmentEvent::Cancelled))
            .await;
    }

    pub(crate) fn mark_running(&mut self) -> Result<(), String> {
        self.status.state = RunState::Running;
        self.status.last_activity = Some("claude running".into());
        self.queue_status(/*force*/ true)
    }

    pub(crate) async fn finalize_success_from_stream(&mut self, terminal: &TerminalResult) {
        apply_terminal_metadata(&mut self.status, terminal);
        self.status.state = RunState::Ok;
        self.status.result = terminal.result_text.clone();
        self.status.error = None;
        self.status.last_activity = Some("complete".into());
        // Stream mapping is metadata-only for `result`; session writes the sole
        // terminal Completed attachment after process exit.
        let _ = self
            .finish_with_barrier(Some(AttachmentEvent::Completed))
            .await;
    }

    pub(crate) async fn finalize_failure_from_stream(
        &mut self,
        terminal: Option<&TerminalResult>,
        detail: String,
        prefer_detail: bool,
    ) {
        if let Some(terminal) = terminal {
            apply_terminal_metadata(&mut self.status, terminal);
        }
        let error = bound_text(if prefer_detail {
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
        });
        self.status.state = RunState::Error;
        self.status.error = Some(error.clone());
        self.status.last_activity = Some("failed".into());
        // Exactly one terminal Failed attachment, owned by session after exit.
        let _ = self
            .finish_with_barrier(Some(AttachmentEvent::Failed(error)))
            .await;
    }

    pub(crate) async fn flush_terminal_status(&mut self) {
        if !self.status.state.is_terminal() || self.closed {
            return;
        }
        let _ = self.finish_with_barrier(None).await;
    }

    /// Flush final status/attachment, then publish one terminal watch update.
    async fn finish_with_barrier(
        &mut self,
        mut terminal_attachment: Option<AttachmentEvent>,
    ) -> Result<(), String> {
        self.take_attachment_feedback();
        if self.closed {
            return self.status_error_result();
        }
        self.closed = true;

        // Sticky status-write failures must win before the terminal attachment
        // is chosen so the journal cannot end in Completed after a durability miss.
        self.apply_sticky_status_error_before_terminal(&mut terminal_attachment);

        let terminal_attachment = if self.attachment_enabled {
            terminal_attachment
        } else {
            None
        };

        let (ack_tx, ack_rx) = oneshot::channel();
        let command = PersistCommand::Barrier {
            status: self.status.clone(),
            terminal_attachment,
            ack: ack_tx,
        };

        let send_ok = if let Some(tx) = self.persist_tx.take() {
            // Blocking send off the runtime so a full queue can still drain.
            // Map inside the closure so the JoinHandle does not carry SendError.
            matches!(
                tokio::task::spawn_blocking(move || tx.send(command).is_ok()).await,
                Ok(true)
            )
        } else {
            false
        };

        if !send_ok {
            self.note_send_failure("status");
            self.apply_local_sticky_demotion();
            // Best-effort: still persist the latest rate-limit observation.
            // Tests inject an explicit path through hooks on the worker; without
            // a live worker, skip default home writes under cfg(test).
            #[cfg(not(test))]
            if let Some(observation) = self.rate_limit_slot.take() {
                if let Ok(path) = super::super::rate_limit::default_state_path() {
                    let _ = super::super::rate_limit::store_observation(&path, observation);
                }
            }
            #[cfg(test)]
            {
                let _ = self.rate_limit_slot.take();
            }
            self.emergency_write_status().await;
            self.join_worker().await;
            self.publish_watch();
            return self.status_error_result();
        }

        let barrier_ack = match ack_rx.await {
            Ok(ack) => ack,
            Err(_) => BarrierAck {
                status: self.status.clone(),
                first_status_error: Some("status persistence worker stopped before barrier".into()),
            },
        };
        self.join_worker().await;
        self.take_attachment_feedback();
        let attachment_error = self.status.attachment_error.clone();

        self.status = barrier_ack.status;
        if self.status.attachment_error.is_none() {
            self.status.attachment_error = attachment_error;
        }
        if let Some(error) = barrier_ack.first_status_error {
            if self.first_status_error.is_none() {
                self.first_status_error = Some(error);
            }
        }
        self.pull_status_error_feedback();
        if self.status.attachment_error.is_some() || self.first_status_error.is_some() {
            self.emergency_write_status().await;
        }
        self.publish_watch();

        self.status_error_result()
    }

    fn apply_local_sticky_demotion(&mut self) {
        self.pull_status_error_feedback();
        if let Some(error) = self.first_status_error.clone() {
            let _ = demote_status_for_sticky_error(&mut self.status, &error);
        }
    }

    fn apply_sticky_status_error_before_terminal(
        &mut self,
        terminal_attachment: &mut Option<AttachmentEvent>,
    ) {
        self.pull_status_error_feedback();
        let Some(error) = self.first_status_error.clone() else {
            return;
        };
        let detail = demote_status_for_sticky_error(&mut self.status, &error);
        demote_completed_attachment(
            terminal_attachment,
            self.status.error.clone().unwrap_or(detail),
        );
    }

    async fn emergency_write_status(&self) {
        let path = self.path.clone();
        let status = self.status.clone();
        let _ = tokio::task::spawn_blocking(move || subagent::write_status(&path, &status)).await;
    }

    async fn join_worker(&mut self) {
        if let Some(join) = self.persist_join.take() {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = join.join();
            })
            .await;
        }
    }

    /// Abort without waiting. Best-effort terminal disk write; no second watch
    /// publish after a finished barrier.
    pub(crate) fn abort_detached(&mut self) {
        let already_closed = self.closed;
        if !self.closed {
            if let Some(tx) = self.persist_tx.take() {
                let _ = tx.try_send(PersistCommand::Abort);
                drop(tx);
            }
            self.closed = true;
        } else {
            self.persist_tx.take();
        }
        if self.status.state.is_terminal() {
            let _ = subagent::write_status(&self.path, &self.status);
            if !already_closed {
                self.publish_watch();
            }
        }
        self.detach_worker_join();
    }

    fn detach_worker_join(&mut self) {
        if let Some(join) = self.persist_join.take() {
            let _ = std::thread::Builder::new()
                .name("rho-claude-persist-join".into())
                .spawn(move || {
                    let _ = join.join();
                });
        }
    }

    pub(crate) async fn shutdown(mut self) {
        if !self.closed {
            let _ = self.finish_with_barrier(None).await;
        } else {
            self.join_worker().await;
        }
    }
}

impl Drop for StatusSink {
    fn drop(&mut self) {
        self.abort_detached();
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
        if let Some(input) = usage.total_input_tokens() {
            status.input_tokens = input;
        }
        if let Some(output) = usage.output_tokens {
            status.output_tokens = output;
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

fn bound_text(text: String) -> String {
    if text.len() <= MAX_SINK_TEXT_BYTES {
        return text;
    }
    let mut cut = MAX_SINK_TEXT_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}
