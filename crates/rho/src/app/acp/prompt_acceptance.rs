//! When a `session/prompt`'s user message joins the conversation.
//!
//! ACP v1 answers a prompt when its turn ends, so nothing is reported early.
//! ACP v2 answers once the message is inserted, with the message's ID, and
//! requires a replay to reuse that ID. Such a prompt's turn saves its
//! message before its run starts (`MessageSave::BeforeRun`) and reports the
//! row a reload replays it as (`SessionEvent::MessageInserted`), as a
//! workflow start reports the row it saved; the ID is the one replay derives
//! from that row. A command that saves nothing, such as `/compact`, is
//! accepted with a fresh ID, since nothing of it is replayed.

use agent_client_protocol::schema::v1::MessageId;

use super::events::EventMapper;
use crate::app::session_runtime::{InsertedMessage, MessageSave};

/// Reports a prompt's user message, once, when the session inserts it.
pub(super) enum PromptAcceptance {
    /// ACP v1: the prompt response reports the turn.
    Implicit,
    /// ACP v2: called with the inserted message's ID before the prompt sends
    /// any of its turn's updates.
    Report(Box<dyn FnOnce(MessageId) + Send + Sync>),
}

impl PromptAcceptance {
    /// When the prompt's next turn saves its user message: before its run
    /// while the prompt still waits to report it, else at the first step.
    pub(super) fn message_save(&self) -> MessageSave {
        match self {
            Self::Implicit => MessageSave::AtFirstStep,
            Self::Report(_) => MessageSave::BeforeRun,
        }
    }

    /// Reports `message` with the ID replay gives it; later calls do
    /// nothing.
    pub(super) fn accept(&mut self, message: InsertedMessage) {
        if let Self::Report(report) = std::mem::replace(self, Self::Implicit) {
            report(match message {
                InsertedMessage::Saved(origin) => EventMapper::saved_message_id(&origin),
                InsertedMessage::Unreplayed => {
                    MessageId::new(format!("rho-user-{}", uuid::Uuid::new_v4()))
                }
            });
        }
    }
}
