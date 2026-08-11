//! Plain-text messaging between a parent session and its delegated agents.
//!
//! Both directions are non-blocking and share one body budget. Child to parent:
//! notices carry a short finding or blocker and deliver at the parent's next
//! turn boundary, the same way background completions do (questionnaires, by
//! contrast, block the child until the parent answers). Parent to child: the
//! parent stages text into the child's steering queue through [`SteeringSlot`],
//! applied at the child's next provider turn.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubagentNotice {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) parent_session_id: SessionId,
    pub(crate) message: String,
}

/// End-to-end budget for accepted but undelivered notices in one parent binding.
///
/// Each [`SubagentNoticeBridge::bind_parent`] installs a fresh generation. A
/// handle cloned from an older binding only mutates that generation's counter,
/// so a late discard after rebind cannot panic or free slots owned by the new
/// parent receiver.
#[derive(Clone)]
pub(crate) struct NoticePermits {
    outstanding: Arc<AtomicUsize>,
}

impl NoticePermits {
    fn new() -> Self {
        Self {
            outstanding: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns slots for notices the parent no longer owes a delivery for.
    pub(crate) fn release(&self, count: usize) {
        if count == 0 {
            return;
        }
        self.outstanding
            .fetch_sub(count, Ordering::AcqRel)
            .checked_sub(count)
            .expect("notice permit release exceeds outstanding reservations");
    }

    fn try_reserve(&self) -> bool {
        let mut current = self.outstanding.load(Ordering::Acquire);
        while current < NOTICE_QUEUE_CAPACITY {
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
/// Sender selection, reservation, and rebinding share one mutex so a post
/// never pairs a stale sender with a replacement generation's permits (or the
/// reverse). Dropping a binding retires its sender; outstanding permits on that
/// generation remain valid for the inbox that accepted those notices.
#[derive(Clone)]
pub(crate) struct SubagentNoticeBridge {
    binding: Arc<Mutex<Option<NoticeBinding>>>,
    capacity: usize,
}

impl Default for SubagentNoticeBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentNoticeBridge {
    pub(crate) fn new() -> Self {
        Self {
            binding: Arc::new(Mutex::new(None)),
            capacity: NOTICE_QUEUE_CAPACITY,
        }
    }

    /// Installs the parent receiver and a fresh permit generation.
    ///
    /// Replaces any previous binding. The returned permits are the only handle
    /// that should free budget for notices accepted on this binding.
    pub(crate) fn bind_parent(&self) -> (mpsc::Receiver<SubagentNotice>, NoticePermits) {
        let (sender, receiver) = mpsc::channel(self.capacity);
        let permits = NoticePermits::new();
        *self.binding_slot() = Some(NoticeBinding {
            sender,
            permits: permits.clone(),
        });
        (receiver, permits)
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
        let (sender, permits) = {
            let guard = self.binding_slot();
            let binding = guard.as_ref().ok_or(NoticePostError::Unbound)?;
            if !binding.permits.try_reserve() {
                return Err(NoticePostError::QueueFull {
                    capacity: NOTICE_QUEUE_CAPACITY,
                });
            }
            (binding.sender.clone(), binding.permits.clone())
        };
        sender.try_send(notice).map_err(|error| {
            permits.release(1);
            match error {
                mpsc::error::TrySendError::Full(_) => NoticePostError::QueueFull {
                    capacity: NOTICE_QUEUE_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => NoticePostError::Unbound,
            }
        })
    }

    fn binding_slot(&self) -> std::sync::MutexGuard<'_, Option<NoticeBinding>> {
        self.binding
            .lock()
            .expect("subagent notice bridge binding lock")
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
