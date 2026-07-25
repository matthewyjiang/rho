//! Synchronous-facing status + attachment sink with a tiny background writer.
//!
//! Callers update in-memory state and enqueue work. One OS thread owns disk I/O
//! so high-volume stream drains never block on `fsync`. Terminal finish waits
//! once for the queue to drain - no emergency dual-write path.

use std::{
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError, SyncSender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use tokio::sync::watch;

use crate::subagent::{self, RunState, RunStatus};

use super::journal::{AttachmentEvent, AttachmentWriter};

/// Longest a status-file write is deferred while text streams.
const REPORT_THROTTLE: Duration = Duration::from_secs(2);
const MAX_STATUS_TEXT_BYTES: usize = 256 * 1024;
const QUEUE_CAPACITY: usize = 256;
const FINISH_JOIN_BUDGET: Duration = Duration::from_secs(5);

/// Identity fields stamped onto every run's `result.json`.
#[derive(Clone, Debug)]
pub(crate) struct RunArtifactIdentity {
    pub(crate) agent_id: String,
    pub(crate) agent_fingerprint: String,
    pub(crate) provider: String,
    pub(crate) model: String,
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
    tx: Option<SyncSender<WriterCommand>>,
    join: Option<JoinHandle<()>>,
}

fn starting_status(identity: &RunArtifactIdentity) -> RunStatus {
    RunStatus {
        state: RunState::Starting,
        agent_id: Some(identity.agent_id.clone()),
        agent_fingerprint: Some(identity.agent_fingerprint.clone()),
        provider: Some(identity.provider.clone()),
        model: Some(identity.model.clone()),
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
        if status.attachment_error.is_some() {
            let _ = subagent::write_status(&path, &status);
        }
        if let Some(tx) = &status_tx {
            tx.send_replace(status.clone());
        }

        let (tx, rx) = mpsc::sync_channel::<WriterCommand>(QUEUE_CAPACITY);
        let worker_path = path.clone();
        let join = std::thread::Builder::new()
            .name("rho-run-artifacts".into())
            .spawn(move || writer_loop(worker_path, attachment, rx))
            .ok();

        Ok(Self {
            path,
            status,
            status_tx,
            last_write: Instant::now(),
            closed: false,
            attachment_enabled,
            tx: Some(tx),
            join,
        })
    }

    /// Publish the current status to the watch channel and disk queue.
    pub(crate) fn publish(&mut self) {
        self.last_write = Instant::now();
        if let Some(tx) = &self.status_tx {
            tx.send_replace(self.status.clone());
        }
        self.enqueue(WriterCommand::Status(self.status.clone()));
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
        if self.closed || !self.attachment_enabled {
            return;
        }
        if !self.enqueue(WriterCommand::Attachment(event)) {
            self.attachment_enabled = false;
            self.status.attachment_error =
                Some("could not record attach output: recording could not keep up".into());
            self.publish();
        }
    }

    pub(crate) fn append_last_text(&mut self, text: &str) {
        let buffer = self.status.last_text.get_or_insert_with(String::new);
        buffer.push_str(text);
        if buffer.len() > MAX_STATUS_TEXT_BYTES {
            let cut = buffer.len() - MAX_STATUS_TEXT_BYTES;
            let boundary = (cut..buffer.len())
                .find(|index| buffer.is_char_boundary(*index))
                .unwrap_or(buffer.len());
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
        if let Some(join) = self.join.take() {
            let deadline = Instant::now() + FINISH_JOIN_BUDGET;
            loop {
                if join.is_finished() {
                    let _ = join.join();
                    break;
                }
                if Instant::now() >= deadline {
                    // Detach: best-effort direct status write so attach sees terminal.
                    let _ = subagent::write_status(&self.path, &self.status);
                    std::thread::spawn(move || {
                        let _ = join.join();
                    });
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        } else {
            let _ = subagent::write_status(&self.path, &self.status);
        }
        if let Some(tx) = &self.status_tx {
            tx.send_replace(self.status.clone());
        }
    }

    fn enqueue(&mut self, command: WriterCommand) -> bool {
        let Some(tx) = self.tx.as_ref() else {
            return false;
        };
        match tx.try_send(command) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(command)) => {
                // Status is replaceable: drop oldest-equivalent by skipping.
                matches!(command, WriterCommand::Status(_))
            }
            Err(mpsc::TrySendError::Disconnected(_)) => false,
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
) {
    // Coalesce replaceable status snapshots so a burst of Running updates does
    // not serialize every write behind the attachment journal.
    let mut pending_status: Option<RunStatus> = None;
    loop {
        let command = if let Some(status) = pending_status.take() {
            match rx.recv_timeout(Duration::from_millis(0)) {
                Ok(command) => {
                    // Keep the latest status; handle the new command below.
                    pending_status = Some(status);
                    command
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = subagent::write_status(&path, &status);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = subagent::write_status(&path, &status);
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
                    let _ = subagent::write_status(&path, &status);
                }
                if let Some(writer) = attachment.as_mut() {
                    if writer.write_event(&event).is_err() {
                        attachment = None;
                    }
                }
            }
            WriterCommand::Finish {
                status,
                terminal_attachment,
            } => {
                if let Some(previous) = pending_status.take() {
                    let _ = subagent::write_status(&path, &previous);
                }
                if let (Some(event), Some(writer)) = (terminal_attachment, attachment.as_mut()) {
                    let _ = writer.write_event(&event);
                }
                let _ = subagent::write_status(&path, &status);
                // Drain anything already queued, then exit.
                while let Ok(extra) = rx.try_recv() {
                    if let WriterCommand::Status(status) = extra {
                        let _ = subagent::write_status(&path, &status);
                    }
                }
                break;
            }
        }
    }
}

fn bound_text(text: String) -> String {
    if text.len() <= MAX_STATUS_TEXT_BYTES {
        return text;
    }
    let mut cut = MAX_STATUS_TEXT_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = text;
    out.truncate(cut);
    out.push('…');
    out
}
