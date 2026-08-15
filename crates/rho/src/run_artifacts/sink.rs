//! Synchronous-facing status + attachment sink with a tiny background writer.
//!
//! Callers update in-memory state and enqueue work. One OS thread owns disk I/O
//! so high-volume stream drains never block on `fsync`. Terminal finish waits
//! once for the queue to drain - no emergency dual-write path.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use tokio::sync::watch;

use rho_sdk::{ceil_char_boundary, floor_char_boundary, ELLIPSIS};

use crate::subagent::{self, RunState, RunStatus};

use super::journal::{AttachmentEvent, AttachmentWriter};

/// Longest a status-file write is deferred while text streams.
const REPORT_THROTTLE: Duration = Duration::from_secs(2);
const MAX_STATUS_TEXT_BYTES: usize = 256 * 1024;
const QUEUE_CAPACITY: usize = 256;
const FINISH_JOIN_BUDGET: Duration = Duration::from_secs(5);
/// Longest a producer waits for the writer to drain before giving up on a
/// journal event. Recording is only disabled after this budget is spent.
///
/// Producers are tokio tasks driving the run's select loop, and a wedged disk
/// makes every event pay the full budget, so this stays short: long enough to
/// ride out a burst the writer is already draining, short enough that a stuck
/// disk cannot make the run stop responding. Windows journal `flush` under
/// load can exceed 250 ms, so that target uses a longer ride-out.
#[cfg(not(windows))]
const ATTACHMENT_ENQUEUE_BUDGET: Duration = Duration::from_millis(250);
#[cfg(windows)]
const ATTACHMENT_ENQUEUE_BUDGET: Duration = Duration::from_secs(2);
/// Upper bound on one coalesced delta so a fast producer cannot grow a single
/// journal line without limit. Leftover deltas stay queued and merge next time.
const MAX_COALESCED_DELTA_BYTES: usize = 64 * 1024;

/// Identity fields stamped onto every run's `result.json`.
#[derive(Clone, Debug)]
pub(crate) struct RunArtifactIdentity {
    pub(crate) agent_id: String,
    pub(crate) agent_fingerprint: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) runtime: crate::agent::AgentRuntime,
}

enum WriterCommand {
    Status(RunStatus),
    Attachment(AttachmentEvent),
    /// Final ordered write, then stop the worker.
    Finish {
        status: RunStatus,
        terminal_attachment: Option<AttachmentEvent>,
    },
}

/// One delegated run's durable status file and attachment journal.
///
/// Both the Rho automation reporter and the Claude session adapter can drive
/// this type so terminal rules and journal layout stay one contract.
pub(crate) struct RunArtifactSink {
    path: PathBuf,
    pub(crate) status: RunStatus,
    status_tx: Option<watch::Sender<RunStatus>>,
    last_write: Instant,
    closed: bool,
    attachment_enabled: bool,
    attachment_error: Arc<Mutex<Option<String>>>,
    /// Shared with the background writer so a failed status update is warned once.
    status_write_failed: Arc<AtomicBool>,
    tx: Option<SyncSender<WriterCommand>>,
    /// Signaled once when the background writer exits.
    done_rx: Option<mpsc::Receiver<()>>,
    join: Option<JoinHandle<()>>,
}

fn starting_status(identity: &RunArtifactIdentity) -> RunStatus {
    RunStatus {
        state: RunState::Starting,
        agent_id: Some(identity.agent_id.clone()),
        agent_fingerprint: Some(identity.agent_fingerprint.clone()),
        provider: Some(identity.provider.clone()),
        model: Some(identity.model.clone()),
        runtime: Some(identity.runtime),
        started_at: Some(subagent::unix_now_secs()),
        last_activity: Some("starting".into()),
        ..RunStatus::default()
    }
}

impl RunArtifactSink {
    /// Open a new run boundary: force-replace prior terminal status, write the
    /// prompt attachment when possible, and publish Starting.
    pub(crate) fn open(
        path: PathBuf,
        identity: &RunArtifactIdentity,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
    ) -> anyhow::Result<Self> {
        let status = starting_status(identity);
        subagent::initialize_status(&path, &status)?;
        Self::from_started(path, status, prompt, status_tx)
    }

    /// Continue after the launcher already wrote the Starting boundary.
    ///
    /// Used when the executor stamps `result.json` before the task runs so the
    /// handle and external attach see identity immediately. Skips a second
    /// force-replace; still creates the journal and background writer.
    pub(crate) fn continue_from(
        path: PathBuf,
        status: RunStatus,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
    ) -> anyhow::Result<Self> {
        Self::from_started(path, status, prompt, status_tx)
    }

    fn from_started(
        path: PathBuf,
        mut status: RunStatus,
        prompt: &str,
        status_tx: Option<watch::Sender<RunStatus>>,
    ) -> anyhow::Result<Self> {
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
        let attachment_error = Arc::new(Mutex::new(status.attachment_error.clone()));
        let status_write_failed = Arc::new(AtomicBool::new(false));
        if status.attachment_error.is_some() {
            write_status_best_effort(&path, &status, &status_write_failed);
        }
        if let Some(tx) = &status_tx {
            tx.send_replace(status.clone());
        }

        let (tx, rx) = mpsc::sync_channel::<WriterCommand>(QUEUE_CAPACITY);
        let (done_tx, done_rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker_write_failed = Arc::clone(&status_write_failed);
        let worker_attachment_error = Arc::clone(&attachment_error);
        let join = std::thread::Builder::new()
            .name("rho-run-artifacts".into())
            .spawn(move || {
                writer_loop(
                    worker_path,
                    attachment,
                    rx,
                    worker_write_failed,
                    worker_attachment_error,
                );
                let _ = done_tx.send(());
            })
            .ok();

        Ok(Self {
            path,
            status,
            status_tx,
            last_write: Instant::now(),
            closed: false,
            attachment_enabled,
            attachment_error,
            status_write_failed,
            tx: Some(tx),
            done_rx: Some(done_rx),
            join,
        })
    }

    /// Publish the current status to the watch channel and disk queue.
    pub(crate) fn publish(&mut self) {
        self.sync_attachment_error();
        self.merge_live_title();
        self.last_write = Instant::now();
        if let Some(tx) = &self.status_tx {
            tx.send_replace(self.status.clone());
        }
        self.enqueue(WriterCommand::Status(self.status.clone()));
    }

    /// Keep a title written by the title task when this sink still has none.
    fn merge_live_title(&mut self) {
        if self.status.title.is_some() {
            return;
        }
        let Some(tx) = &self.status_tx else {
            return;
        };
        self.status.title = tx.borrow().title.clone();
    }

    /// Publish when the throttle window has elapsed (streaming text).
    pub(crate) fn publish_throttled(&mut self) {
        if self.last_write.elapsed() >= REPORT_THROTTLE {
            self.publish();
        }
    }

    pub(crate) fn mark_running(&mut self, activity: impl Into<String>) {
        if self.status.state.is_terminal() {
            return;
        }
        self.status.state = RunState::Running;
        self.status.last_activity = Some(activity.into());
        self.publish();
    }

    /// Append one journal event. Attachment failures are sticky on status.
    pub(crate) fn write_attachment(&mut self, event: AttachmentEvent) {
        self.sync_attachment_error();
        if self.closed || !self.attachment_enabled {
            return;
        }
        if !self.enqueue(WriterCommand::Attachment(event)) {
            self.attachment_enabled = false;
            let error = "could not record attach output: recording could not keep up".to_string();
            self.status.attachment_error = Some(error.clone());
            if let Ok(mut attachment_error) = self.attachment_error.lock() {
                *attachment_error = Some(error);
            }
            self.publish();
        }
    }

    /// Journal the event and publish status at the cadence its variant earns.
    ///
    /// Every event is journalled so `rho attach` replay stays complete. Only the
    /// high-frequency streaming variants (text and reasoning deltas, streaming
    /// tool card updates) take the throttled publish: at fast token rates or
    /// with a chatty tool, one status write per event floods the writer and
    /// stalls the journal behind it. Everything else publishes immediately so a
    /// polling host sees state changes without waiting out the throttle window.
    ///
    /// Matched exhaustively on purpose: a new high-frequency variant folded into
    /// a wildcard would silently restore the per-event status write this avoids.
    pub(crate) fn record_attachment(&mut self, event: AttachmentEvent) {
        let throttled = match &event {
            AttachmentEvent::AssistantTextDelta(_)
            | AttachmentEvent::ReasoningDelta(_)
            | AttachmentEvent::ToolUpdated { .. } => true,
            AttachmentEvent::Prompt(_)
            | AttachmentEvent::ToolStarted { .. }
            | AttachmentEvent::ToolFinished { .. }
            | AttachmentEvent::Notice(_)
            | AttachmentEvent::ContextUsage(_)
            | AttachmentEvent::Usage(_)
            | AttachmentEvent::StepStarted
            | AttachmentEvent::ProviderStreamReset
            | AttachmentEvent::Completed
            | AttachmentEvent::Cancelled
            | AttachmentEvent::Failed(_) => false,
        };
        // Journal first: `write_attachment` publishes itself when recording
        // fails, and that failure status must not be pre-empted by this one.
        self.write_attachment(event);
        if throttled {
            self.publish_throttled();
        } else {
            self.publish();
        }
    }

    pub(crate) fn append_last_text(&mut self, text: &str) {
        let buffer = self.status.last_text.get_or_insert_with(String::new);
        buffer.push_str(text);
        if buffer.len() > MAX_STATUS_TEXT_BYTES {
            let cut = buffer.len() - MAX_STATUS_TEXT_BYTES;
            let boundary = ceil_char_boundary(buffer, cut);
            buffer.drain(..boundary);
        }
    }

    /// Finish successfully. Idempotent once terminal.
    pub(crate) fn finish_ok(&mut self, result: Option<String>) {
        if self.status.state.is_terminal() {
            return;
        }
        self.status.state = RunState::Ok;
        if let Some(result) = result {
            self.status.result = Some(bound_text(result));
        }
        self.status.last_activity = Some("completed".into());
        self.status.mark_finished_now();
        self.finish(Some(AttachmentEvent::Completed));
    }

    /// Finish with a hard failure. Idempotent once terminal.
    pub(crate) fn finish_error(&mut self, error: impl Into<String>) {
        if self.status.state.is_terminal() {
            return;
        }
        let error = bound_text(error.into());
        self.status.state = RunState::Error;
        self.status.error = Some(error.clone());
        self.status.last_activity = Some("failed".into());
        self.status.mark_finished_now();
        self.finish(Some(AttachmentEvent::Failed(error)));
    }

    /// Finish cancelled / stopped. Idempotent once terminal.
    pub(crate) fn finish_stopped(&mut self, reason: impl Into<String>) {
        if self.status.state.is_terminal() {
            return;
        }
        self.status.state = RunState::Stopped;
        self.status.last_activity = Some(reason.into());
        if self.status.result.is_none() {
            if let Some(text) = &self.status.last_text {
                self.status.result = Some(format!("(partial, stopped before finishing)\n{text}"));
            }
        }
        self.status.mark_finished_now();
        self.finish(Some(AttachmentEvent::Cancelled));
    }

    fn finish(&mut self, terminal_attachment: Option<AttachmentEvent>) {
        self.closed = true;
        let terminal_attachment = if self.attachment_enabled {
            terminal_attachment
        } else {
            None
        };
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(WriterCommand::Finish {
                status: self.status.clone(),
                terminal_attachment,
            });
            // Dropping the sender closes the queue after Finish.
            drop(tx);
        }
        let done_rx = self.done_rx.take();
        if let Some(join) = self.join.take() {
            let finished = match done_rx {
                Some(done_rx) => !matches!(
                    done_rx.recv_timeout(FINISH_JOIN_BUDGET),
                    Err(RecvTimeoutError::Timeout)
                ),
                // No completion signal (thread failed to start wiring): fall back to join budget.
                None => {
                    let deadline = Instant::now() + FINISH_JOIN_BUDGET;
                    while !join.is_finished() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    join.is_finished()
                }
            };
            if finished {
                let _ = join.join();
            } else {
                // Detach: best-effort direct status write so attach sees terminal,
                // then publish again after the worker exits so attachment_error
                // recorded during Finish still reaches watch subscribers.
                write_status_best_effort(&self.path, &self.status, &self.status_write_failed);
                let path = self.path.clone();
                let mut status = self.status.clone();
                let status_tx = self.status_tx.clone();
                let attachment_error = Arc::clone(&self.attachment_error);
                let status_write_failed = Arc::clone(&self.status_write_failed);
                std::thread::spawn(move || {
                    let _ = join.join();
                    if let Some(error) =
                        attachment_error.lock().ok().and_then(|error| error.clone())
                    {
                        status.attachment_error = Some(error);
                    }
                    write_status_best_effort(&path, &status, &status_write_failed);
                    if let Some(tx) = status_tx {
                        tx.send_replace(status);
                    }
                });
            }
        } else {
            write_status_best_effort(&self.path, &self.status, &self.status_write_failed);
        }
        self.sync_attachment_error();
        if let Some(tx) = &self.status_tx {
            tx.send_replace(self.status.clone());
        }
    }

    fn sync_attachment_error(&mut self) {
        let error = self
            .attachment_error
            .lock()
            .ok()
            .and_then(|error| error.clone());
        if let Some(error) = error {
            self.attachment_enabled = false;
            self.status.attachment_error = Some(error);
        }
    }

    /// Hand one command to the writer thread, reporting whether it was accepted.
    ///
    /// Invariants when the queue is full:
    /// - `Status` is replaceable, so a skipped snapshot is not a failure; the
    ///   next publish carries the newer state.
    /// - `Attachment` is not replaceable, so the producer blocks for at most
    ///   [`ATTACHMENT_ENQUEUE_BUDGET`] instead of dropping the event. Losing a
    ///   burst race must slow the run down, not silently truncate the journal.
    fn enqueue(&mut self, command: WriterCommand) -> bool {
        let Some(tx) = self.tx.as_ref() else {
            return false;
        };
        match tx.try_send(command) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(WriterCommand::Status(_))) => true,
            Err(mpsc::TrySendError::Full(command)) => send_within_budget(tx, command),
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }
}

/// Retry a full queue until the writer drains a slot or the budget runs out.
///
/// `SyncSender::send` alone would wait without limit, which can pin a runtime
/// worker behind a pathological disk; `send_timeout` is still unstable. Retrying
/// keeps the wait bounded, and it only ever costs time on the overflow path.
///
/// The writer drains coalesced deltas in gulps, so the common overflow clears in
/// microseconds: the first [`SPIN_ATTEMPTS`] retries only yield the thread, and
/// the 1 ms sleep starts once a slot is clearly not coming back that fast.
fn send_within_budget(tx: &SyncSender<WriterCommand>, command: WriterCommand) -> bool {
    /// Yield-only retries before falling back to sleeping.
    const SPIN_ATTEMPTS: u32 = 16;
    const RETRY_INTERVAL: Duration = Duration::from_millis(1);

    let deadline = Instant::now() + ATTACHMENT_ENQUEUE_BUDGET;
    let mut command = command;
    let mut attempt = 0_u32;
    loop {
        match tx.try_send(command) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Full(returned)) => {
                if Instant::now() >= deadline {
                    return false;
                }
                command = returned;
                if attempt < SPIN_ATTEMPTS {
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(RETRY_INTERVAL);
                }
                attempt = attempt.saturating_add(1);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
        }
    }
}

impl Drop for RunArtifactSink {
    fn drop(&mut self) {
        if !self.closed {
            if !self.status.state.is_terminal() {
                // Panic/abort path: mark stopped rather than inventing an error
                // when the run never left Starting.
                if self.status.state == RunState::Starting {
                    self.status.state = RunState::Stopped;
                    self.status.last_activity = Some("run dropped before start completed".into());
                } else {
                    self.status.state = RunState::Error;
                    if self.status.error.is_none() {
                        self.status.error = Some("run ended without a terminal status".into());
                    }
                }
            }
            self.finish(None);
        }
    }
}

fn writer_loop(
    path: PathBuf,
    mut attachment: Option<AttachmentWriter>,
    rx: mpsc::Receiver<WriterCommand>,
    status_write_failed: Arc<AtomicBool>,
    attachment_error: Arc<Mutex<Option<String>>>,
) {
    // Coalesce replaceable status snapshots so a burst of Running updates does
    // not serialize every write behind the attachment journal.
    let mut pending_status: Option<RunStatus> = None;
    // Set when delta coalescing pulled a command it must not consume; that
    // command is handled first on the next iteration so journal order matches
    // enqueue order for every event type.
    let mut held: Option<WriterCommand> = None;
    loop {
        let command = if let Some(command) = held.take() {
            command
        } else if let Some(status) = pending_status.take() {
            match rx.recv_timeout(Duration::from_millis(0)) {
                Ok(command) => {
                    // Keep the latest status; handle the new command below.
                    pending_status = Some(status);
                    command
                }
                Err(RecvTimeoutError::Timeout) => {
                    write_status_best_effort(&path, &status, &status_write_failed);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    write_status_best_effort(&path, &status, &status_write_failed);
                    break;
                }
            }
        } else {
            match rx.recv() {
                Ok(command) => command,
                Err(_) => break,
            }
        };

        match command {
            WriterCommand::Status(status) => {
                pending_status = Some(status);
            }
            WriterCommand::Attachment(event) => {
                if let Some(status) = pending_status.take() {
                    write_status_best_effort(&path, &status, &status_write_failed);
                }
                let event = coalesce_adjacent_deltas(event, &rx, &mut held);
                if let Some(writer) = attachment.as_mut() {
                    if let Err(error) = writer.write_event(&event) {
                        record_attachment_error(&attachment_error, error);
                        attachment = None;
                    }
                }
            }
            WriterCommand::Finish {
                mut status,
                terminal_attachment,
            } => {
                if let Some(previous) = pending_status.take() {
                    write_status_best_effort(&path, &previous, &status_write_failed);
                }
                if let (Some(event), Some(writer)) = (terminal_attachment, attachment.as_mut()) {
                    if let Err(error) = writer.write_event(&event) {
                        record_attachment_error(&attachment_error, error);
                    }
                }
                if let Some(error) = attachment_error.lock().ok().and_then(|error| error.clone()) {
                    status.attachment_error = Some(error);
                }
                write_status_best_effort(&path, &status, &status_write_failed);
                // Drain anything already queued, then exit.
                while let Ok(extra) = rx.try_recv() {
                    if let WriterCommand::Status(status) = extra {
                        write_status_best_effort(&path, &status, &status_write_failed);
                    }
                }
                break;
            }
        }
    }
}

/// Which streaming text variant a run of deltas belongs to.
///
/// Assistant text and reasoning are separate streams, so only same-kind deltas
/// merge.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeltaKind {
    AssistantText,
    Reasoning,
}

impl DeltaKind {
    fn into_event(self, text: String) -> AttachmentEvent {
        match self {
            Self::AssistantText => AttachmentEvent::AssistantTextDelta(text),
            Self::Reasoning => AttachmentEvent::ReasoningDelta(text),
        }
    }
}

/// Merge text deltas queued back-to-back into one journal write.
///
/// Invariants:
/// - Content is lossless and ordered: merging only concatenates neighbouring
///   deltas of the same kind, so a replaying reader sees the same text with
///   fewer events.
/// - Any other command, including a delta of the other kind, ends the run and is
///   returned through `held` so it is written next, unconsumed.
/// - The merged text stops growing at [`MAX_COALESCED_DELTA_BYTES`].
fn coalesce_adjacent_deltas(
    first: AttachmentEvent,
    rx: &mpsc::Receiver<WriterCommand>,
    held: &mut Option<WriterCommand>,
) -> AttachmentEvent {
    let (kind, mut text) = match first {
        AttachmentEvent::AssistantTextDelta(text) => (DeltaKind::AssistantText, text),
        AttachmentEvent::ReasoningDelta(text) => (DeltaKind::Reasoning, text),
        other => return other,
    };
    while text.len() < MAX_COALESCED_DELTA_BYTES {
        let Ok(command) = rx.try_recv() else {
            break;
        };
        match command {
            WriterCommand::Attachment(AttachmentEvent::AssistantTextDelta(next))
                if kind == DeltaKind::AssistantText =>
            {
                text.push_str(&next);
            }
            WriterCommand::Attachment(AttachmentEvent::ReasoningDelta(next))
                if kind == DeltaKind::Reasoning =>
            {
                text.push_str(&next);
            }
            other => {
                *held = Some(other);
                break;
            }
        }
    }
    kind.into_event(text)
}

fn record_attachment_error(attachment_error: &Mutex<Option<String>>, error: anyhow::Error) {
    if let Ok(mut recorded) = attachment_error.lock() {
        recorded.get_or_insert_with(|| format!("could not record attach output: {error}"));
    }
}

/// Attached hosts poll the status file, so an unreported write failure freezes
/// the run state they observe. Warn once per sink and keep remaining writes
/// best-effort without flooding the terminal.
fn write_status_best_effort(path: &Path, status: &RunStatus, status_write_failed: &AtomicBool) {
    if let Err(error) = subagent::write_status(path, status) {
        if status_write_failed.swap(true, Ordering::Relaxed) {
            return;
        }
        eprintln!(
            "warning: could not update run status {}: {error}",
            path.display()
        );
    }
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;

fn bound_text(text: String) -> String {
    if text.len() <= MAX_STATUS_TEXT_BYTES {
        return text;
    }
    let cut = floor_char_boundary(&text, MAX_STATUS_TEXT_BYTES);
    let mut out = text;
    out.truncate(cut);
    out.push_str(ELLIPSIS);
    out
}
