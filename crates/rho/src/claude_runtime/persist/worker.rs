//! Durable worker protocol and OS-thread runner for Claude run artifacts.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc as std_mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
};

#[cfg(test)]
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar,
};

use tokio::sync::oneshot;

use crate::{
    subagent::{self, RunState, RunStatus},
    tui::{AttachmentEvent, AttachmentWriter},
};

use super::super::rate_limit::{self, RateLimitSlot};

pub(super) const PERSISTENCE_QUEUE_CAPACITY: usize = 64;

/// Hooks for stalling, failing, or logging durable writes without disk sleeps.
///
/// The type exists in every build so persistence signatures do not change with
/// build mode; outside tests it carries no fields and does nothing.
#[derive(Clone, Default)]
pub(crate) struct PersistHooks {
    #[cfg(test)]
    pub(crate) stall: Option<Arc<WriterStall>>,
    /// Stall after barrier ack is sent so join can time out while work is done.
    #[cfg(test)]
    pub(crate) post_barrier_stall: Option<Arc<WriterStall>>,
    #[cfg(test)]
    pub(crate) fail_status_writes: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_attachment_writes: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) log: Arc<Mutex<Vec<PersistLogEntry>>>,
    #[cfg(test)]
    pub(crate) rate_limit_path: Option<std::path::PathBuf>,
}

impl PersistHooks {
    /// Block while a scenario holds the writer. No-op outside tests.
    fn wait(&self) {
        #[cfg(test)]
        if let Some(stall) = &self.stall {
            stall.wait_if_stalled();
        }
    }

    /// Consume one injected status-write failure. Always `None` outside tests.
    fn take_status_failure(&self) -> Option<String> {
        #[cfg(test)]
        if self.fail_status_writes.load(Ordering::SeqCst) > 0 {
            self.fail_status_writes.fetch_sub(1, Ordering::SeqCst);
            return Some("injected status write failure".into());
        }
        None
    }

    /// Consume one injected attachment-write failure. `None` outside tests.
    fn take_attachment_failure(&self) -> Option<String> {
        #[cfg(test)]
        if self.fail_attachment_writes.load(Ordering::SeqCst) > 0 {
            self.fail_attachment_writes.fetch_sub(1, Ordering::SeqCst);
            return Some("injected attachment write failure".into());
        }
        None
    }

    /// Where a scenario wants rate-limit state written. Tests only; production
    /// always uses the default home path.
    fn rate_limit_path(&self) -> Option<&std::path::Path> {
        #[cfg(test)]
        return self.rate_limit_path.as_deref();
        #[cfg(not(test))]
        None
    }

    #[cfg(test)]
    fn note(&self, entry: PersistLogEntry) {
        if let Ok(mut log) = self.log.lock() {
            log.push(entry);
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct WriterStall {
    lock: Mutex<bool>,
    cv: Condvar,
}

#[cfg(test)]
impl WriterStall {
    pub(crate) fn new_stalled() -> Arc<Self> {
        Arc::new(Self {
            lock: Mutex::new(true),
            cv: Condvar::new(),
        })
    }

    pub(crate) fn release(&self) {
        let mut stalled = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *stalled = false;
        self.cv.notify_all();
    }

    pub(crate) fn wait_if_stalled(&self) {
        let mut stalled = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *stalled {
            stalled = self
                .cv
                .wait(stalled)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PersistLogEntry {
    Status { force: bool, state: RunState },
    Attachment(AttachmentKind),
    RateLimit,
    BarrierDone,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttachmentKind {
    Prompt,
    AssistantTextDelta,
    Other,
    Completed,
    Failed,
    Cancelled,
}

#[cfg(test)]
impl AttachmentKind {
    fn from_event(event: &AttachmentEvent) -> Self {
        match event {
            AttachmentEvent::Prompt(_) => Self::Prompt,
            AttachmentEvent::AssistantTextDelta(_) => Self::AssistantTextDelta,
            AttachmentEvent::Completed => Self::Completed,
            AttachmentEvent::Failed(_) => Self::Failed,
            AttachmentEvent::Cancelled => Self::Cancelled,
            _ => Self::Other,
        }
    }
}

pub(super) enum PersistCommand {
    Status {
        status: RunStatus,
        force: bool,
    },
    Attachment(AttachmentEvent),
    /// Wake the worker so a coalesced rate-limit observation can flush.
    FlushRateLimit,
    /// Final ordered write: status, optional terminal attachment, then stop.
    Barrier {
        status: RunStatus,
        terminal_attachment: Option<AttachmentEvent>,
        ack: oneshot::Sender<BarrierAck>,
    },
    /// Stop without a terminal barrier (session abort/drop).
    Abort,
}

/// Final status + sticky error after ordered barrier flush.
pub(super) struct BarrierAck {
    pub(super) status: RunStatus,
    pub(super) first_status_error: Option<String>,
}

#[derive(Default)]
pub(super) struct PersistFeedback {
    pub(super) first_status_error: Option<String>,
    pub(super) attachment_error: Option<String>,
}

struct PersistWorker {
    path: std::path::PathBuf,
    attachment: Option<AttachmentWriter>,
    last_written: Option<RunStatus>,
    feedback: Arc<Mutex<PersistFeedback>>,
    rate_limit_slot: Arc<RateLimitSlot>,
    abandon: Arc<AtomicBool>,
    hooks: PersistHooks,
}

impl PersistWorker {
    fn abandoned(&self) -> bool {
        self.abandon.load(AtomicOrdering::SeqCst)
    }

    /// First status-write failure recorded so far, if any.
    fn sticky_status_error(&self) -> Option<String> {
        self.feedback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .first_status_error
            .clone()
    }

    fn run(mut self, rx: std_mpsc::Receiver<PersistCommand>) {
        let mut pending = VecDeque::new();
        loop {
            if self.abandoned() {
                // Still publish the latest rate-limit observation on abandon so
                // transcript stalls cannot drop it after Abort/timeout.
                self.flush_rate_limit();
                break;
            }
            let next = match pending.pop_front() {
                Some(command) => command,
                None => match rx.recv() {
                    Ok(command) => command,
                    Err(_) => {
                        self.flush_rate_limit();
                        break;
                    }
                },
            };
            if self.abandoned() {
                // Sink already performed emergency terminal write; do not race
                // status/attachment, but still flush coalesced rate-limit state.
                self.flush_rate_limit();
                break;
            }
            // Flush coalesced rate-limit state whenever the worker is awake so a
            // saturated transcript queue cannot indefinitely delay the latest
            // observation.
            self.flush_rate_limit();
            let command = coalesce_nonterminal_status(next, &rx, &mut pending);
            match command {
                PersistCommand::Status { status, force } => {
                    if !self.abandoned() {
                        self.perform_status(status, force);
                    }
                }
                PersistCommand::Attachment(event) => {
                    if !self.abandoned() {
                        self.perform_attachment(event);
                    }
                }
                PersistCommand::FlushRateLimit => {
                    // Already flushed at the top of the loop.
                }
                PersistCommand::Barrier {
                    status,
                    terminal_attachment,
                    ack,
                } => {
                    let status = self.perform_barrier(status, terminal_attachment);
                    // Terminal barrier owns the last rate-limit flush for the run.
                    self.flush_rate_limit();
                    let _ = ack.send(BarrierAck {
                        status,
                        first_status_error: self.sticky_status_error(),
                    });
                    #[cfg(test)]
                    if let Some(stall) = &self.hooks.post_barrier_stall {
                        stall.wait_if_stalled();
                    }
                    break;
                }
                PersistCommand::Abort => {
                    self.flush_rate_limit();
                    break;
                }
            }
        }
    }

    fn flush_rate_limit(&mut self) {
        let Some(observation) = self.rate_limit_slot.take() else {
            return;
        };
        #[cfg(test)]
        self.hooks.note(PersistLogEntry::RateLimit);
        self.hooks.wait();
        // Tests without an explicit path skip home-directory writes entirely.
        match self.hooks.rate_limit_path() {
            Some(path) => {
                let _ = rate_limit::store_observation(path, observation);
            }
            #[cfg(not(test))]
            None => {
                if let Ok(path) = rate_limit::default_state_path() {
                    let _ = rate_limit::store_observation(&path, observation);
                }
            }
            #[cfg(test)]
            None => {}
        }
    }

    fn perform_status(&mut self, status: RunStatus, force: bool) {
        #[cfg(test)]
        self.hooks.note(PersistLogEntry::Status {
            force,
            state: status.state,
        });
        self.hooks.wait();
        if self.abandoned() {
            return;
        }
        // Worker-local monotonicity mirrors `subagent::write_status`: never
        // demote an already-terminal snapshot with a queued nonterminal update.
        if let Some(last) = &self.last_written {
            if last.state.is_terminal() && !status.state.is_terminal() {
                return;
            }
        }
        if !force && self.last_written.as_ref() == Some(&status) {
            return;
        }
        if let Some(error) = self.hooks.take_status_failure() {
            self.record_status_error(error);
            return;
        }
        match subagent::write_status(&self.path, &status) {
            Ok(()) => {
                self.last_written = Some(status);
            }
            Err(error) => {
                self.record_status_error(error.to_string());
            }
        }
    }

    /// Ordered terminal flush: status, then the terminal attachment.
    ///
    /// Returns the status to ack, which is the demoted one whenever a sticky
    /// status-write failure means Ok/Completed can no longer be claimed. Any
    /// abandon observed between writes stops early with what is durable so far;
    /// the sink has already performed its own emergency write in that case.
    fn perform_barrier(
        &mut self,
        status: RunStatus,
        terminal_attachment: Option<AttachmentEvent>,
    ) -> RunStatus {
        if self.abandoned() {
            return status;
        }
        // Sticky first, then terminal write; re-resolve if that write fails.
        let (status, terminal_attachment) =
            self.resolve_barrier_terminal(status, terminal_attachment);
        self.perform_status(status.clone(), /*force*/ true);
        if self.abandoned() {
            return status;
        }

        let (status, terminal_attachment) =
            self.resolve_barrier_terminal(status, terminal_attachment);
        if matches!(status.state, RunState::Error | RunState::Stopped)
            && self
                .last_written
                .as_ref()
                .is_none_or(|last| last.state != status.state || last.error != status.error)
        {
            self.perform_status(status.clone(), /*force*/ true);
        }
        if self.abandoned() {
            return status;
        }

        if let Some(event) = terminal_attachment {
            self.perform_attachment(event);
        }
        #[cfg(test)]
        self.hooks.note(PersistLogEntry::BarrierDone);
        status
    }

    /// Demote Ok/Completed when sticky status writes failed. Disk is caller's job.
    fn resolve_barrier_terminal(
        &mut self,
        mut status: RunStatus,
        mut terminal_attachment: Option<AttachmentEvent>,
    ) -> (RunStatus, Option<AttachmentEvent>) {
        let Some(error) = self.sticky_status_error() else {
            return (status, terminal_attachment);
        };
        let detail = demote_status_for_sticky_error(&mut status, &error);
        demote_completed_attachment(
            &mut terminal_attachment,
            status.error.clone().unwrap_or(detail),
        );
        (status, terminal_attachment)
    }

    fn perform_attachment(&mut self, event: AttachmentEvent) {
        #[cfg(test)]
        self.hooks
            .note(PersistLogEntry::Attachment(AttachmentKind::from_event(
                &event,
            )));
        self.hooks.wait();
        if self.abandoned() {
            return;
        }
        // Recording already disabled: never consume an injected failure here.
        if self.attachment.is_none() {
            return;
        }
        if let Some(error) = self.hooks.take_attachment_failure() {
            self.attachment = None;
            self.record_attachment_error(error);
            return;
        }
        let Some(writer) = self.attachment.as_mut() else {
            return;
        };
        match writer.write_event(&event) {
            Ok(()) => {}
            Err(error) => {
                self.attachment = None;
                self.record_attachment_error(error.to_string());
            }
        }
    }

    fn record_status_error(&self, error: String) {
        let mut feedback = self
            .feedback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if feedback.first_status_error.is_none() {
            feedback.first_status_error = Some(error);
        }
    }

    fn record_attachment_error(&self, error: String) {
        let mut feedback = self
            .feedback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if feedback.attachment_error.is_none() {
            feedback.attachment_error = Some(format!("could not record attach output: {error}"));
        }
    }
}

fn coalesce_nonterminal_status(
    first: PersistCommand,
    rx: &std_mpsc::Receiver<PersistCommand>,
    pending: &mut VecDeque<PersistCommand>,
) -> PersistCommand {
    let PersistCommand::Status {
        mut status,
        force: false,
    } = first
    else {
        return first;
    };
    loop {
        match rx.try_recv() {
            Ok(PersistCommand::Status {
                status: next,
                force: false,
            }) => {
                status = next;
            }
            Ok(other) => {
                pending.push_back(other);
                break;
            }
            Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected) => break,
        }
    }
    PersistCommand::Status {
        status,
        force: false,
    }
}

pub(super) fn spawn_persist_worker(
    path: std::path::PathBuf,
    attachment: Option<AttachmentWriter>,
    last_written: Option<RunStatus>,
    rate_limit_slot: Arc<RateLimitSlot>,
    abandon: Arc<AtomicBool>,
    hooks: PersistHooks,
) -> (
    std_mpsc::SyncSender<PersistCommand>,
    Arc<Mutex<PersistFeedback>>,
    JoinHandle<()>,
) {
    let (tx, rx) = std_mpsc::sync_channel(PERSISTENCE_QUEUE_CAPACITY);
    let feedback = Arc::new(Mutex::new(PersistFeedback::default()));
    let feedback_worker = Arc::clone(&feedback);
    let join = std::thread::Builder::new()
        .name("rho-claude-persist".into())
        .spawn(move || {
            PersistWorker {
                path,
                attachment,
                last_written,
                feedback: feedback_worker,
                rate_limit_slot,
                abandon,
                hooks,
            }
            .run(rx);
        })
        .expect("spawn claude persist worker");
    (tx, feedback, join)
}

#[derive(Clone, Debug)]
pub(crate) struct ClaudeRunIdentity {
    pub(crate) agent_id: String,
    pub(crate) agent_fingerprint: String,
    pub(crate) model: Option<String>,
}

pub(super) fn demote_status_for_sticky_error(status: &mut RunStatus, error: &str) -> String {
    let detail = format!("claude code: status persistence failed: {error}");
    match status.state {
        RunState::Starting | RunState::Running | RunState::Ok => {
            status.state = RunState::Error;
            status.last_activity = Some("failed".into());
            if status.error.is_none() {
                status.error = Some(detail.clone());
            }
        }
        RunState::Error | RunState::Stopped => {
            if status.error.is_none() {
                status.error = Some(detail.clone());
            }
        }
    }
    detail
}

pub(super) fn demote_completed_attachment(
    terminal_attachment: &mut Option<AttachmentEvent>,
    failed_detail: String,
) {
    if matches!(terminal_attachment, Some(AttachmentEvent::Completed)) {
        *terminal_attachment = Some(AttachmentEvent::Failed(failed_detail));
    }
}
