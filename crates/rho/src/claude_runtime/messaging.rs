//! Parent → Claude-cli child messaging over stream-json stdin.
//!
//! Delegated Claude runs keep stdin open with `--input-format stream-json`.
//! The parent posts plain text through a bounded channel; the drain writer
//! encodes each body as one NDJSON user turn. Closing stdin after the child
//! emits a terminal `result` (with no pending parent messages) ends the run.
//!
//! Terminal shutdown seals the port before the final drain so a concurrent
//! `agents message` cannot be acknowledged and then dropped.

use crate::cli_runtime::parent_messages::{frame_parent_message, ParentMessageInbox};
use std::future::Future;
use std::pin::Pin;

use serde_json::json;
use tokio::sync::mpsc;

use crate::cli_runtime::drain::{FollowUp, FollowUpSource};
use crate::cli_runtime::stream_effect::StreamEffect;
use crate::presentation::{parent_message_card, NotificationDelivery};
use crate::run_artifacts::AttachmentEvent;

/// Drain-side adapter: encodes queued parent text as stream-json user turns.
pub(crate) struct ClaudeFollowUpSource {
    inbox: ParentMessageInbox,
}

impl ClaudeFollowUpSource {
    pub(crate) fn new(inbox: ParentMessageInbox) -> Self {
        Self { inbox }
    }

    fn encode(text: String) -> FollowUp {
        let line = encode_user_turn(&frame_parent_message(&text));
        FollowUp {
            line,
            written: Some(StreamEffect::Attachment(AttachmentEvent::Message(
                Box::new(parent_message_card(
                    text,
                    NotificationDelivery::Queued,
                    "written to Claude stdin; awaiting its next turn".into(),
                )),
            ))),
        }
    }
}

impl FollowUpSource for ClaudeFollowUpSource {
    fn try_recv(&mut self) -> Result<FollowUp, mpsc::error::TryRecvError> {
        let text = self.inbox.try_recv()?;
        Ok(Self::encode(text))
    }

    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Option<FollowUp>> + Send + '_>> {
        Box::pin(async {
            let text = self.inbox.recv().await?;
            Some(Self::encode(text))
        })
    }

    fn seal(&self) {
        self.inbox.seal();
    }
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

#[cfg(test)]
#[path = "messaging_tests.rs"]
mod tests;
