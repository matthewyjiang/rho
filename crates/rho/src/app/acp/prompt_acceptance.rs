//! When a `session/prompt`'s user message joins the conversation.
//!
//! ACP v1 answers a prompt when its turn ends, so nothing is reported early.
//! ACP v2 answers once the message is inserted, with the message's ID, and
//! requires a replay to reuse that ID. The session runtime reports when the
//! message is inserted and the row a reload replays it as (see
//! `session_runtime::MessageAcceptance`); the ID is the one replay derives
//! from that row. A command that saves nothing, such as `/compact`, is
//! accepted with a fresh ID, since nothing of it is replayed.

use agent_client_protocol::schema::v1::MessageId;

use super::events::EventMapper;
use crate::app::session_runtime::{InsertedMessage, MessageAcceptance};

/// Reports a prompt's user message, once, when the session inserts it.
pub(super) enum PromptAcceptance {
    /// ACP v1: the prompt response reports the turn.
    Implicit,
    /// ACP v2: called with the inserted message's ID before the prompt sends
    /// any of its turn's updates.
    Report(Box<dyn FnOnce(MessageId) + Send + Sync>),
}

impl PromptAcceptance {
    /// The runtime's acceptance, naming the inserted message as replay will.
    pub(super) fn into_runtime(self) -> MessageAcceptance {
        match self {
            Self::Implicit => MessageAcceptance::Implicit,
            Self::Report(report) => MessageAcceptance::Report(Box::new(move |message| {
                report(match message {
                    InsertedMessage::Saved(origin) => EventMapper::saved_message_id(&origin),
                    InsertedMessage::Unreplayed => {
                        MessageId::new(format!("rho-user-{}", uuid::Uuid::new_v4()))
                    }
                })
            })),
        }
    }
}
