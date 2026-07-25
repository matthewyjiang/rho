//! StatusSink orchestration for one Claude CLI run.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
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
use super::worker::{
    demote_completed_attachment, demote_status_for_sticky_error, spawn_persist_worker, BarrierAck,
    ClaudeRunIdentity, PersistCommand, PersistFeedback,
};
#[cfg(test)]
use super::worker::{PersistHooks, WriterStall};

const MAX_SINK_TEXT_BYTES: usize = 256 * 1024;

/// Explicit deadlines for terminal persistence shutdown work.
///
/// Production defaults favor durability. Tests inject short budgets so
/// `finish_with_barrier` cannot hang the async runtime.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PersistShutdownBudgets {
    pub(crate) queue_send: Duration,
    pub(crate) barrier_ack: Duration,
    pub(crate) worker_join: Duration,
    pub(crate) emergency_write: Duration,
}

impl Default for PersistShutdownBudgets {
    fn default() -> Self {
        Self {
            queue_send: Duration::from_secs(2),
            barrier_ack: Duration::from_secs(5),
            worker_join: Duration::from_secs(2),
            emergency_write: Duration::from_secs(2),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
struct EmergencyWriteHooks {
    stall: Option<Arc<WriterStall>>,
    fail_writes: Arc<std::sync::atomic::AtomicUsize>,
}

/// In-memory + durable sink for one Claude CLI run.
pub(crate) struct StatusSink {
    path: std::path::PathBuf,
    pub(crate) status: RunStatus,
    status_tx: Option<watch::Sender<RunStatus>>,
    persist_tx: Option<std_mpsc::SyncSender<PersistCommand>>,
    persist_join: Option<JoinHandle<()>>,
    feedback: Arc<Mutex<PersistFeedback>>,
    rate_limit_slot: Arc<RateLimitSlot>,
    /// Set when shutdown abandons the worker so later barrier writes cannot
    /// overwrite an emergency terminal status.
    abandon: Arc<AtomicBool>,
    shutdown_budgets: PersistShutdownBudgets,
    attachment_enabled: bool,
    first_status_error: Option<String>,
    closed: bool,
    #[cfg(test)]
    emergency_hooks: EmergencyWriteHooks,
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
            Self::build(
                path,
                identity,
                prompt,
                status_tx,
                PersistHooks::default(),
                PersistShutdownBudgets::default(),
                EmergencyWriteHooks::default(),
            )
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
        Self::build(
            path,
            identity,
            prompt,
            status_tx,
            hooks,
            PersistShutdownBudgets::default(),
            EmergencyWriteHooks::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_hooks_and_budgets(
        path: std::path::PathBuf,
        identity: &ClaudeRunIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        hooks: PersistHooks,
        budgets: PersistShutdownBudgets,
    ) -> anyhow::Result<Self> {
        Self::build(
            path,
            identity,
            prompt,
            status_tx,
            hooks,
            budgets,
            EmergencyWriteHooks::default(),
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_shutdown_hooks(
        path: std::path::PathBuf,
        identity: &ClaudeRunIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        hooks: PersistHooks,
        budgets: PersistShutdownBudgets,
        emergency_stall: Option<Arc<WriterStall>>,
        emergency_fail_writes: Arc<std::sync::atomic::AtomicUsize>,
    ) -> anyhow::Result<Self> {
        Self::build(
            path,
            identity,
            prompt,
            status_tx,
            hooks,
            budgets,
            EmergencyWriteHooks {
                stall: emergency_stall,
                fail_writes: emergency_fail_writes,
            },
        )
    }

    fn build(
        path: std::path::PathBuf,
        identity: &ClaudeRunIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
        #[cfg(test)] hooks: PersistHooks,
        #[cfg(test)] budgets: PersistShutdownBudgets,
        #[cfg(test)] emergency_hooks: EmergencyWriteHooks,
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
        let abandon = Arc::new(AtomicBool::new(false));
        let (persist_tx, feedback, persist_join) = spawn_persist_worker(
            path.clone(),
            attachment,
            Some(status.clone()),
            Arc::clone(&rate_limit_slot),
            Arc::clone(&abandon),
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
            abandon,
            #[cfg(test)]
            shutdown_budgets: budgets,
            #[cfg(not(test))]
            shutdown_budgets: PersistShutdownBudgets::default(),
            attachment_enabled,
            first_status_error: None,
            closed: false,
            #[cfg(test)]
            emergency_hooks,
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
    ///
    /// Every blocking phase has an explicit budget from
    /// [`PersistShutdownBudgets`]. On timeout the sink abandons the worker
    /// (non-blocking), records a diagnostic, best-effort-writes terminal
    /// status under its own budget, and returns.
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

        let send_result = if let Some(tx) = self.persist_tx.take() {
            self.send_barrier_bounded(tx, command).await
        } else {
            BarrierSendResult::Disconnected
        };

        match send_result {
            BarrierSendResult::Sent => {}
            BarrierSendResult::Full => {
                self.note_shutdown_failure(
                    "status persistence barrier could not be queued before deadline",
                );
                return self.finish_after_shutdown_failure().await;
            }
            BarrierSendResult::Disconnected => {
                self.note_send_failure("status");
                return self.finish_after_shutdown_failure().await;
            }
        }

        let barrier_ack = match tokio::time::timeout(self.shutdown_budgets.barrier_ack, ack_rx)
            .await
        {
            Ok(Ok(ack)) => ack,
            Ok(Err(_)) => BarrierAck {
                status: self.status.clone(),
                first_status_error: Some("status persistence worker stopped before barrier".into()),
            },
            Err(_) => {
                self.note_shutdown_failure("status persistence barrier acknowledgment timed out");
                return self.finish_after_shutdown_failure().await;
            }
        };

        if !self.join_worker_bounded().await {
            self.note_shutdown_failure("status persistence worker join timed out");
            // Barrier already acked final status; keep that snapshot and only
            // detach the join so the async task cannot hang.
            self.abandon_worker(/*detach_sender*/ false);
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
            return self.status_error_result();
        }

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

    async fn send_barrier_bounded(
        &self,
        tx: std_mpsc::SyncSender<PersistCommand>,
        mut command: PersistCommand,
    ) -> BarrierSendResult {
        let deadline = Instant::now() + self.shutdown_budgets.queue_send;
        loop {
            match tx.try_send(command) {
                Ok(()) => return BarrierSendResult::Sent,
                Err(std_mpsc::TrySendError::Disconnected(_)) => {
                    return BarrierSendResult::Disconnected;
                }
                Err(std_mpsc::TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        // Drop the sender without a detached blocked thread so
                        // a stalled worker cannot pin queue resources forever.
                        drop(tx);
                        return BarrierSendResult::Full;
                    }
                    command = returned;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }
    }

    async fn finish_after_shutdown_failure(&mut self) -> Result<(), String> {
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
        self.abandon_worker(/*detach_sender*/ true);
        self.emergency_write_status().await;
        self.publish_watch();
        self.status_error_result()
    }

    fn note_shutdown_failure(&mut self, message: &str) {
        if self.first_status_error.is_none() {
            self.first_status_error = Some(message.to_string());
        }
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
        #[cfg(test)]
        let emergency_hooks = self.emergency_hooks.clone();
        // Dedicated OS thread: a stalled write must not pin Tokio's blocking pool.
        let (done_tx, done_rx) = oneshot::channel();
        let spawn_ok = std::thread::Builder::new()
            .name("rho-claude-persist-emergency".into())
            .spawn(move || {
                #[cfg(test)]
                {
                    if let Some(stall) = &emergency_hooks.stall {
                        stall.wait_if_stalled();
                    }
                    if emergency_hooks.fail_writes.load(Ordering::SeqCst) > 0 {
                        emergency_hooks.fail_writes.fetch_sub(1, Ordering::SeqCst);
                        let _ = done_tx.send(Err(std::io::Error::other(
                            "injected emergency write failure",
                        )));
                        return;
                    }
                }
                let result = subagent::write_status(&path, &status);
                let _ = done_tx.send(result);
            })
            .is_ok();
        if !spawn_ok {
            return;
        }
        match tokio::time::timeout(self.shutdown_budgets.emergency_write, done_rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(_))) | Ok(Err(_)) | Err(_) => {
                // Best-effort only. The in-memory/watch status still carries the
                // terminal snapshot even when disk cannot be updated in budget.
            }
        }
    }

    /// Wait up to the join budget. Returns false when the worker must be abandoned.
    async fn join_worker_bounded(&mut self) -> bool {
        let Some(join) = self.persist_join.take() else {
            return true;
        };
        // Use an OS helper thread rather than `spawn_blocking` so a worker that
        // never exits cannot pin a Tokio blocking-pool slot forever.
        let (done_tx, done_rx) = oneshot::channel();
        let spawn_ok = std::thread::Builder::new()
            .name("rho-claude-persist-join".into())
            .spawn(move || {
                let _ = join.join();
                let _ = done_tx.send(());
            })
            .is_ok();
        if !spawn_ok {
            // Helper spawn failed; treat as abandon without blocking.
            return false;
        }
        matches!(
            tokio::time::timeout(self.shutdown_budgets.worker_join, done_rx).await,
            Ok(Ok(()))
        )
    }

    fn abandon_worker(&mut self, detach_sender: bool) {
        self.abandon.store(true, Ordering::SeqCst);
        if detach_sender {
            // Dropping the sender unblocks a worker sitting in recv without
            // creating a blocked sender thread.
            self.persist_tx.take();
        }
        self.detach_worker_join();
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
        self.abandon.store(true, Ordering::SeqCst);
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
        } else if !self.join_worker_bounded().await {
            self.abandon_worker(/*detach_sender*/ false);
        }
    }

    #[cfg(test)]
    pub(crate) fn first_status_error_for_test(&self) -> Option<String> {
        self.first_status_error.clone()
    }
}

enum BarrierSendResult {
    Sent,
    Full,
    Disconnected,
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
