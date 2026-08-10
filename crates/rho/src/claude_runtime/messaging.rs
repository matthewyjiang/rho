//! Parent → Claude-cli child messaging over stream-json stdin.
//!
//! Delegated Claude runs keep stdin open with `--input-format stream-json`.
//! The parent posts plain text through a bounded channel; the drain writer
//! encodes each body as one NDJSON user turn. Closing stdin after the child
//! emits a terminal `result` (with no pending parent messages) ends the run.

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
    sender: mpsc::Sender<String>,
}

impl ClaudeMessageHandle {
    /// Stages a validated plain-text parent message for the Claude child.
    pub(crate) async fn send(&self, text: String) -> Result<(), ClaudeMessageSendError> {
        self.sender
            .send(text)
            .await
            .map_err(|_| ClaudeMessageSendError::Closed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ClaudeMessageSendError {
    #[error("delegated Claude run is no longer accepting parent messages")]
    Closed,
}

/// Creates a parent-message port and the drain-side receiver.
pub(crate) fn message_channel() -> (ClaudeMessageHandle, mpsc::Receiver<String>) {
    let (sender, receiver) = mpsc::channel(PARENT_MESSAGE_QUEUE_CAPACITY);
    (ClaudeMessageHandle { sender }, receiver)
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
