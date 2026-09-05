//! Plain-text messaging between a parent session and its delegated agents.
//!
//! Both directions are non-blocking and share one body budget. Child to parent:
//! notices carry a short finding or blocker and deliver at the parent's next
//! safe runtime boundary, the same way background completions do (questionnaires, by
//! contrast, block the child until the parent answers). Parent to child: the
//! parent stages text into the child's steering queue through [`SteeringSlot`],
//! applied at the child's next provider turn.

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use rho_sdk::SessionId;
use tokio::sync::mpsc;

/// Queue depth for child->parent notices waiting on the parent session.
///
/// Enough for a burst of parallel children; fail loud when a child floods the
/// parent instead of growing without bound. The budget is end-to-end: accepted
/// notices still count after the TUI drains them out of the transport channel
/// until the parent delivers or discards them.
pub(crate) const NOTICE_QUEUE_CAPACITY: usize = 32;

/// Soft cap on one plain-text message body, in either direction.
///
/// Redirects and findings stay short; dumping a transcript belongs in the run
/// result, not a parent turn. One budget for both directions so there is a
/// single visible tripwire.
pub(crate) const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// A trimmed, non-empty message body that fits [`MAX_MESSAGE_BYTES`].
///
/// Tools parse one at their own argument boundary so an over-budget body is
/// reported as invalid arguments once, rather than surfacing as an execution
/// failure deeper in the send path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValidatedMessage(String);

impl ValidatedMessage {
    /// Trims and budget-checks a caller-supplied body.
    pub(crate) fn parse(text: &str) -> Result<Self, MessageValidationError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(MessageValidationError::Empty);
        }
        let bytes = trimmed.len();
        if bytes > MAX_MESSAGE_BYTES {
            return Err(MessageValidationError::TooLarge {
                bytes,
                max_bytes: MAX_MESSAGE_BYTES,
            });
        }
        Ok(Self(trimmed.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum MessageValidationError {
    #[error("message text must not be empty")]
    Empty,
    #[error("message text is {bytes} bytes; limit is {max_bytes} bytes")]
    TooLarge { bytes: usize, max_bytes: usize },
}

/// One plain-text notice raised by a delegated run for its parent.
#[derive(Clone, Debug)]
pub(crate) struct SubagentNotice {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) parent_session_id: SessionId,
    pub(crate) message: String,
    pub(crate) acknowledged: Arc<AtomicBool>,
}

impl PartialEq for SubagentNotice {
    fn eq(&self, other: &Self) -> bool {
        (
            &self.run_id,
            &self.agent_id,
            &self.parent_session_id,
            &self.message,
        ) == (
            &other.run_id,
            &other.agent_id,
            &other.parent_session_id,
            &other.message,
        )
    }
}
impl Eq for SubagentNotice {}

impl SubagentNotice {
    pub(crate) fn acknowledge(&self) {
        self.acknowledged.store(true, Ordering::Release);
    }
    pub(crate) fn is_acknowledged(&self) -> bool {
        self.acknowledged.load(Ordering::Acquire)
    }
}

/// End-to-end budget for accepted but undelivered notices in one parent binding.
///
/// Each [`SubagentNoticeBridge::rebind_parent`] installs a fresh generation. A
/// handle cloned from an older binding only mutates that generation's counter,
/// so a late discard after rebind cannot panic or free slots owned by the new
/// parent receiver.
#[derive(Clone)]
pub(crate) struct NoticePermits {
    outstanding: Arc<AtomicUsize>,
    pending: Arc<Mutex<Vec<SubagentNotice>>>,
}

impl NoticePermits {
    fn new() -> Self {
        Self {
            outstanding: Arc::new(AtomicUsize::new(0)),
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns slots for notices the parent no longer owes a delivery for.
    ///
    /// Validates the outstanding count before mutating so a bad release cannot
    /// wrap the counter even briefly before panicking.
    pub(crate) fn release(&self, count: usize) {
        if count == 0 {
            return;
        }
        self.outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_sub(count)
            })
            .expect("notice permit release exceeds outstanding reservations");
    }

    /// Releases the receipt as well as its capacity slot. The shared receipts
    /// let explicit terminal status include notices already queued by the UI.
    pub(crate) fn release_notice(&self, notice: &SubagentNotice) {
        let mut pending = self.pending.lock().expect("pending notice receipts");
        if let Some(index) = pending
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.acknowledged, &notice.acknowledged))
        {
            pending.remove(index);
        }
        self.release(1);
    }

    fn try_reserve(&self, capacity: usize) -> bool {
        let mut current = self.outstanding.load(Ordering::Acquire);
        while current < capacity {
            match self.outstanding.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
        false
    }

    #[cfg(test)]
    pub(crate) fn outstanding(&self) -> usize {
        self.outstanding.load(Ordering::Acquire)
    }
}

/// Live parent binding: the sender and the permit generation that owns its budget.
struct NoticeBinding {
    sender: mpsc::Sender<SubagentNotice>,
    permits: NoticePermits,
}

/// Child→parent notice transport with a generation-scoped end-to-end budget.
///
/// Sender selection, reservation, enqueue, and rebinding share one mutex so a
/// post never pairs a stale sender with a replacement generation's permits (or
/// the reverse), and never enqueues on a binding that a concurrent rebind has
/// already replaced. Dropping a binding retires its sender; outstanding permits
/// on that generation remain valid for the inbox that accepted those notices.
#[derive(Clone)]
pub(crate) struct SubagentNoticeBridge {
    binding: Arc<Mutex<Option<NoticeBinding>>>,
    pending: Arc<Mutex<Vec<SubagentNotice>>>,
    capacity: usize,
}

impl Default for SubagentNoticeBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of replacing the parent notice binding.
pub(crate) struct NoticeRebind {
    pub(crate) receiver: mpsc::Receiver<SubagentNotice>,
    pub(crate) permits: NoticePermits,
    /// Notices drained from the retired receiver under the binding lock.
    pub(crate) retained: Vec<SubagentNotice>,
    /// Permit generation that accepted [`Self::retained`] and any notices the
    /// inbox already held from the prior binding. `None` when there was no
    /// previous binding.
    pub(crate) retired_permits: Option<NoticePermits>,
}

impl SubagentNoticeBridge {
    pub(crate) fn new() -> Self {
        Self {
            binding: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(Vec::new())),
            capacity: NOTICE_QUEUE_CAPACITY,
        }
    }

    /// Installs the parent receiver and a fresh permit generation.
    ///
    /// Test convenience over [`Self::rebind_parent`] with no prior receiver.
    /// Production rebinds must pass the old receiver so in-flight notices are
    /// retained under the binding lock.
    #[cfg(test)]
    pub(crate) fn bind_parent(&self) -> (mpsc::Receiver<SubagentNotice>, NoticePermits) {
        let rebind = self.rebind_parent(None);
        (rebind.receiver, rebind.permits)
    }

    /// Atomically drains `old_receiver`, retires it, and installs a new binding.
    ///
    /// Holds the binding lock for the whole swap so a concurrent [`Self::post`]
    /// cannot return `Ok` for a notice that this replacement would discard.
    /// Callers must keep [`NoticeRebind::retained`] deliverable against
    /// [`NoticeRebind::retired_permits`].
    pub(crate) fn rebind_parent(
        &self,
        mut old_receiver: Option<mpsc::Receiver<SubagentNotice>>,
    ) -> NoticeRebind {
        let mut guard = self.binding_slot();
        let retired_permits = guard.as_ref().map(|binding| binding.permits.clone());

        let mut retained = Vec::new();
        if let Some(receiver) = old_receiver.as_mut() {
            while let Ok(notice) = receiver.try_recv() {
                retained.push(notice);
            }
        }
        // Drop the retired receiver while the lock is held so no post can still
        // target it after we install the replacement.
        drop(old_receiver);

        let (sender, receiver) = mpsc::channel(self.capacity);
        let mut permits = NoticePermits::new();
        permits.pending = Arc::clone(&self.pending);
        *guard = Some(NoticeBinding {
            sender,
            permits: permits.clone(),
        });
        NoticeRebind {
            receiver,
            permits,
            retained,
            retired_permits,
        }
    }

    /// Drops the parent binding so later child notices fail closed.
    ///
    /// Outstanding permits on the retired generation stay usable by any inbox
    /// still holding accepted notices from that binding.
    pub(crate) fn unbind_parent(&self) {
        *self.binding_slot() = None;
    }

    /// True while an interactive parent is listening.
    pub(crate) fn is_bound(&self) -> bool {
        self.binding_slot().is_some()
    }

    /// Posts a notice for the parent. Fails when unbound or the queue is full.
    pub(crate) fn post(&self, notice: SubagentNotice) -> Result<(), NoticePostError> {
        self.post_with_enqueue_gap(notice, &|| {})
    }

    /// Like [`Self::post`], but runs `gap` after reserving a slot on the live
    /// binding and before enqueueing.
    ///
    /// Reservation and enqueue share the binding lock so a concurrent rebind
    /// cannot install a replacement (and let the caller drop the retired
    /// receiver) between them. Tests pass a non-empty `gap` to force that
    /// interleaving point with explicit synchronization.
    pub(crate) fn post_with_enqueue_gap(
        &self,
        notice: SubagentNotice,
        gap: &dyn Fn(),
    ) -> Result<(), NoticePostError> {
        let capacity = self.capacity;
        let _delivery = super::notification_delivery::lock();
        let guard = self.binding_slot();
        let binding = guard.as_ref().ok_or(NoticePostError::Unbound)?;
        if !binding.permits.try_reserve(capacity) {
            return Err(NoticePostError::QueueFull { capacity });
        }
        // Hold the binding lock across the gap and enqueue. Releasing it after
        // reserve and before try_send lets rebind install a replacement while
        // the old receiver is still live; try_send then returns Ok for a notice
        // receiver replacement is about to discard.
        gap();
        self.pending
            .lock()
            .expect("pending notice receipts")
            .push(notice.clone());
        binding.sender.try_send(notice.clone()).map_err(|error| {
            binding.permits.release_notice(&notice);
            match error {
                mpsc::error::TrySendError::Full(_) => NoticePostError::QueueFull { capacity },
                mpsc::error::TrySendError::Closed(_) => NoticePostError::Unbound,
            }
        })?;
        Ok(())
    }

    pub(crate) fn pending_for_run(&self, run_id: &str) -> Vec<SubagentNotice> {
        // Posting and the terminal snapshot cannot see a half-enqueued receipt.
        let _binding = self.binding_slot();
        self.pending
            .lock()
            .expect("pending notice receipts")
            .iter()
            .filter(|notice| notice.run_id == run_id)
            .cloned()
            .collect()
    }

    fn binding_slot(&self) -> std::sync::MutexGuard<'_, Option<NoticeBinding>> {
        self.binding
            .lock()
            .expect("subagent notice bridge binding lock")
    }

    /// True when another thread holds the binding lock (reserve/enqueue or rebind).
    #[cfg(test)]
    fn binding_lock_held(&self) -> bool {
        match self.binding.try_lock() {
            Ok(_) => false,
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                panic!("subagent notice bridge binding lock poisoned: {poisoned}")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NoticePostError {
    #[error("delegated agent notices require an interactive parent session listening for them")]
    Unbound,
    #[error(
        "parent notice queue is full ({capacity} waiting); deliver pending notices before sending more"
    )]
    QueueFull { capacity: usize },
}

/// Posts a short plain-text notice from a delegated child to its parent.
///
/// `message_parent` relies on this to stay non-blocking: implementors must not
/// wait on the parent session, and must return [`NoticePostError`] whenever the
/// notice cannot be accepted (unbound parent, full queue, or equivalent).
pub(crate) trait NoticePoster: Send + Sync {
    fn post(&self, message: ValidatedMessage) -> Result<(), NoticePostError>;
}

/// Publishes a delegated Rho run's steering port for the whole live window.
///
/// The slot is empty while the child is still starting and again once the run
/// finishes, so a parent message outside that window fails loud instead of
/// vanishing into a run that will never apply it.
#[derive(Clone, Debug, Default)]
pub(crate) struct SteeringSlot {
    handle: Arc<Mutex<Option<rho_sdk::SteeringHandle>>>,
}

impl SteeringSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Opens the live window once the child session has a run.
    pub(crate) fn publish(&self, handle: rho_sdk::SteeringHandle) {
        *self.slot() = Some(handle);
    }

    /// Closes the live window so late parent messages fail closed.
    pub(crate) fn clear(&self) {
        *self.slot() = None;
    }

    /// Live steering port, or `None` outside the window.
    pub(crate) fn handle(&self) -> Option<rho_sdk::SteeringHandle> {
        self.slot().clone()
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<rho_sdk::SteeringHandle>> {
        self.handle.lock().expect("delegated steering slot lock")
    }
}

/// Frames a parent message so the child treats it as a course correction
/// rather than a fresh task.
pub(crate) fn parent_message_prompt(message: &ValidatedMessage) -> String {
    format!(
        "Message from the parent session (not a new task - incorporate this into your current work):\n\n{}",
        message.as_str()
    )
}

/// Renders queued child notices as one model prompt and one display line set.
pub(crate) fn notice_prompts(notices: &[SubagentNotice]) -> (String, String) {
    let model = notices
        .iter()
        .map(|notice| {
            format!(
                "Message from delegated agent {} ({}):\n{}",
                notice.run_id, notice.agent_id, notice.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let display = notices
        .iter()
        .map(|notice| format!("agent {} ({}) notice", notice.run_id, notice.agent_id))
        .collect::<Vec<_>>()
        .join("\n");
    (model, display)
}

#[cfg(test)]
#[path = "subagent_messaging_tests.rs"]
mod tests;
