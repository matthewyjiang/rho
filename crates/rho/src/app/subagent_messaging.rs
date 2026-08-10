//! Plain-text messaging between a parent session and its delegated agents.
//!
//! Both directions are non-blocking and share one body budget. Child to parent:
//! notices carry a short finding or blocker and deliver at the parent's next
//! turn boundary, the same way background completions do (questionnaires, by
//! contrast, block the child until the parent answers). Parent to child: the
//! parent stages text into the child's steering queue through [`SteeringSlot`],
//! applied at the child's next provider turn.

use std::sync::{Arc, Mutex};

use rho_sdk::SessionId;
use tokio::sync::mpsc;

use super::parent_bridge::ParentBridge;

/// Queue depth for child->parent notices waiting on the parent session.
///
/// Enough for a burst of parallel children; fail loud when a child floods the
/// parent instead of growing without bound.
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

#[derive(Clone)]
pub(crate) struct SubagentNoticeBridge {
    bridge: ParentBridge<SubagentNotice>,
}

impl Default for SubagentNoticeBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentNoticeBridge {
    pub(crate) fn new() -> Self {
        Self {
            bridge: ParentBridge::new(NOTICE_QUEUE_CAPACITY),
        }
    }

    /// Installs the parent receiver. Replaces any previous binding.
    pub(crate) fn bind_parent(&self) -> mpsc::Receiver<SubagentNotice> {
        self.bridge.bind_parent()
    }

    /// Drops the parent binding so later child notices fail closed.
    pub(crate) fn unbind_parent(&self) {
        self.bridge.unbind_parent();
    }

    /// True while an interactive parent is listening.
    pub(crate) fn is_bound(&self) -> bool {
        self.bridge.is_bound()
    }

    /// Posts a notice for the parent. Fails when unbound or the queue is full.
    pub(crate) fn post(&self, notice: SubagentNotice) -> Result<(), NoticePostError> {
        let sender = self.bridge.sender().ok_or(NoticePostError::Unbound)?;
        sender.try_send(notice).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => NoticePostError::QueueFull {
                capacity: NOTICE_QUEUE_CAPACITY,
            },
            mpsc::error::TrySendError::Closed(_) => NoticePostError::Unbound,
        })
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
