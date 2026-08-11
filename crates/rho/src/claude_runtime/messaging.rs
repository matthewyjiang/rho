//! Parent → Claude-cli child messaging over stream-json stdin.
//!
//! Delegated Claude runs keep stdin open with `--input-format stream-json`.
//! The parent posts plain text through a bounded channel; the drain writer
//! encodes each body as one NDJSON user turn. Closing stdin after the child
//! emits a terminal `result` (with no pending parent messages) ends the run.
//!
//! Terminal shutdown seals the port before the final drain so a concurrent
//! `agents message` cannot be acknowledged and then dropped.

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::mpsc;

/// How many parent messages may wait while Claude is mid-turn.
///
/// Claude queues stdin turns; a small buffer is enough for course-corrections
/// without letting a stuck child grow an unbounded backlog.
pub(crate) const PARENT_MESSAGE_QUEUE_CAPACITY: usize = 8;

/// Cloneable port the executor keeps on the live handle.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeMessageHandle {
    /// `None` means the drain has sealed the port for terminal shutdown.
    gate: Arc<Mutex<Option<mpsc::Sender<String>>>>,
}

impl ClaudeMessageHandle {
    /// Stages a validated plain-text parent message for the Claude child.
    ///
    /// Fails closed once the drain seals the port or the receiver is gone. A
    /// successful return means the writer still held a live sender clone and
    /// accepted the body into the queue (or will write it from an in-flight
    /// clone before disconnect).
    pub(crate) async fn send(&self, text: String) -> Result<(), ClaudeMessageSendError> {
        let sender = {
            let guard = self.gate.lock().expect("claude message gate");
            guard.clone().ok_or(ClaudeMessageSendError::Closed)?
        };
        sender
            .send(text)
            .await
            .map_err(|_| ClaudeMessageSendError::Closed)
    }

    /// Test hook: clone the live sender the same way [`Self::send`] does before
    /// awaiting the enqueue, so seal/in-flight interleaving can be forced.
    #[cfg(test)]
    fn clone_sender_for_test(&self) -> Option<mpsc::Sender<String>> {
        self.gate.lock().expect("claude message gate").clone()
    }
}

/// Drain-side inbox paired with [`ClaudeMessageHandle`].
pub(crate) struct ClaudeMessageInbox {
    gate: Arc<Mutex<Option<mpsc::Sender<String>>>>,
    receiver: mpsc::Receiver<String>,
}

impl ClaudeMessageInbox {
    /// Stop accepting new parent sends before the final drain.
    ///
    /// Drops the stored sender so later [`ClaudeMessageHandle::send`] calls
    /// fail immediately. In-flight sends that already cloned the sender can
    /// still enqueue; [`Self::recv`] waits until those clones drop.
    pub(crate) fn seal(&self) {
        let mut guard = self.gate.lock().expect("claude message gate");
        *guard = None;
    }

    pub(crate) fn try_recv(&mut self) -> Result<String, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub(crate) async fn recv(&mut self) -> Option<String> {
        self.receiver.recv().await
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ClaudeMessageSendError {
    #[error("delegated Claude run is no longer accepting parent messages")]
    Closed,
}

/// Creates a parent-message port and the drain-side receiver.
pub(crate) fn message_channel() -> (ClaudeMessageHandle, ClaudeMessageInbox) {
    let (sender, receiver) = mpsc::channel(PARENT_MESSAGE_QUEUE_CAPACITY);
    let gate = Arc::new(Mutex::new(Some(sender)));
    (
        ClaudeMessageHandle {
            gate: Arc::clone(&gate),
        },
        ClaudeMessageInbox { gate, receiver },
    )
}

/// Encodes one parent (or initial prompt) body as a stream-json user turn.
///
/// Format matches Claude Code's stdin protocol:
/// `{"type":"user","message":{"role":"user","content":"..."}}`
pub(crate) fn encode_user_turn(text: &str) -> String {
    let mut line = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": text,
        },
    })
    .to_string();
    line.push('\n');
    line
}

/// Frames a parent course-correction the same way Rho-runtime steering does.
pub(crate) fn frame_parent_message(text: &str) -> String {
    format!(
        "Message from the parent session (not a new task - incorporate this into your current work):\n\n{text}"
    )
}

#[cfg(test)]
#[path = "messaging_tests.rs"]
mod tests;
