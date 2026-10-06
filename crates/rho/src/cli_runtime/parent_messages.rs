//! Bounded parent → child text queue, independent of the child's wire protocol.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Existing Claude receipt: eight course-corrections provide a small backlog
/// without letting a stuck child grow an unbounded queue.
pub(crate) const PARENT_MESSAGE_QUEUE_CAPACITY: usize = 8;

/// Cloneable port retained by the executor for a live delegated run.
#[derive(Clone, Debug)]
pub(crate) struct ParentMessageHandle {
    gate: Arc<Mutex<Option<mpsc::Sender<String>>>>,
}

impl ParentMessageHandle {
    /// Accept a body while the port is live. Enqueueing happens under the gate
    /// lock, so a successful return means the consumer will see the body: no
    /// sender clone outlives the lock to race a seal or a final-empty close.
    pub(crate) fn send(&self, text: String) -> Result<(), ParentMessageSendError> {
        let gate = self.gate.lock().expect("parent message gate");
        let sender = gate.as_ref().ok_or(ParentMessageSendError::Closed)?;
        sender.try_send(text).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => ParentMessageSendError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => ParentMessageSendError::Closed,
        })
    }
}

/// Consumer paired with a parent-message port.
pub(crate) struct ParentMessageInbox {
    gate: Arc<Mutex<Option<mpsc::Sender<String>>>>,
    receiver: mpsc::Receiver<String>,
}

impl ParentMessageInbox {
    /// Reject later sends; bodies already queued remain drainable through recv.
    pub(crate) fn seal(&self) {
        *self.gate.lock().expect("parent message gate") = None;
    }

    /// The next queued body, or close the port when none is queued. Atomic
    /// with [`ParentMessageHandle::send`]: a racing send is either returned
    /// here (and the port stays open) or rejected as closed.
    pub(crate) fn take_next_or_close(&mut self) -> Option<String> {
        let mut gate = self.gate.lock().expect("parent message gate");
        match self.receiver.try_recv() {
            Ok(text) => Some(text),
            Err(_) => {
                *gate = None;
                None
            }
        }
    }

    pub(crate) fn try_recv(&mut self) -> Result<String, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub(crate) async fn recv(&mut self) -> Option<String> {
        self.receiver.recv().await
    }
}

impl Drop for ParentMessageInbox {
    fn drop(&mut self) {
        self.seal();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ParentMessageSendError {
    #[error("delegated run is no longer accepting parent messages")]
    Closed,
    #[error("parent message queue is full: limit {PARENT_MESSAGE_QUEUE_CAPACITY} pending messages; wait for the child to take one")]
    QueueFull,
}

pub(crate) fn message_channel() -> (ParentMessageHandle, ParentMessageInbox) {
    let (sender, receiver) = mpsc::channel(PARENT_MESSAGE_QUEUE_CAPACITY);
    let gate = Arc::new(Mutex::new(Some(sender)));
    (
        ParentMessageHandle {
            gate: Arc::clone(&gate),
        },
        ParentMessageInbox { gate, receiver },
    )
}

/// Same framing as Rho-runtime steering: a correction, not a new task.
pub(crate) fn frame_parent_message(text: &str) -> String {
    format!("Message from the parent session (not a new task - incorporate this into your current work):\n\n{text}")
}

#[cfg(test)]
#[path = "parent_messages_tests.rs"]
mod tests;
