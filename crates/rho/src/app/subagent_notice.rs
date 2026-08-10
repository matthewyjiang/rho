//! Non-blocking notices from delegated agents to their parent session.
//!
//! Questionnaires block a child until the parent answers. Notices only carry a
//! short plain-text finding or blocker and deliver at the parent's next turn
//! boundary, the same way background completions do.

use std::sync::{Arc, Mutex};

use rho_sdk::SessionId;
use tokio::sync::mpsc;

/// Queue depth for child→parent notices waiting on the parent session.
///
/// Mirrors the questionnaire bridge: enough for a burst of parallel children,
/// fail loud when a child floods the parent instead of growing without bound.
pub(crate) const NOTICE_QUEUE_CAPACITY: usize = 32;

/// Soft cap on one notice body. Redirects and findings stay short; dumping a
/// transcript belongs in the run result, not the parent turn.
pub(crate) const MAX_NOTICE_BYTES: usize = 8 * 1024;

/// Soft cap on parent→child message bodies. Same budget as notices so both
/// directions share one visible tripwire.
pub(crate) const MAX_PARENT_MESSAGE_BYTES: usize = 8 * 1024;

/// One plain-text notice raised by a delegated run for its parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubagentNotice {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) parent_session_id: SessionId,
    pub(crate) message: String,
}

#[derive(Clone, Default)]
pub(crate) struct SubagentNoticeBridge {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    sender: Mutex<Option<mpsc::Sender<SubagentNotice>>>,
}

impl SubagentNoticeBridge {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Installs the parent receiver. Replaces any previous binding.
    pub(crate) fn bind_parent(&self) -> mpsc::Receiver<SubagentNotice> {
        let (sender, receiver) = mpsc::channel(NOTICE_QUEUE_CAPACITY);
        *self
            .inner
            .sender
            .lock()
            .expect("subagent notice bridge lock") = Some(sender);
        receiver
    }

    /// Drops the parent binding so later child notices fail closed.
    pub(crate) fn unbind_parent(&self) {
        *self
            .inner
            .sender
            .lock()
            .expect("subagent notice bridge lock") = None;
    }

    /// True while an interactive parent is listening.
    pub(crate) fn is_bound(&self) -> bool {
        self.inner
            .sender
            .lock()
            .expect("subagent notice bridge lock")
            .is_some()
    }

    /// Posts a notice for the parent. Fails when unbound or the queue is full.
    pub(crate) fn post(&self, notice: SubagentNotice) -> Result<(), NoticePostError> {
        let sender = self
            .inner
            .sender
            .lock()
            .expect("subagent notice bridge lock")
            .clone()
            .ok_or(NoticePostError::Unbound)?;
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
    fn post(&self, message: String) -> Result<(), NoticePostError>;
}

/// Validates and trims a plain-text agent message body.
pub(crate) fn validate_message_text(
    text: &str,
    max_bytes: usize,
) -> Result<String, MessageValidationError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(MessageValidationError::Empty);
    }
    let bytes = trimmed.len();
    if bytes > max_bytes {
        return Err(MessageValidationError::TooLarge { bytes, max_bytes });
    }
    Ok(trimmed.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum MessageValidationError {
    #[error("message text must not be empty")]
    Empty,
    #[error("message text is {bytes} bytes; limit is {max_bytes} bytes")]
    TooLarge { bytes: usize, max_bytes: usize },
}

#[cfg(test)]
#[path = "subagent_notice_tests.rs"]
mod tests;
