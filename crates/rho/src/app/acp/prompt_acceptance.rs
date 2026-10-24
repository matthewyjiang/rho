//! When a `session/prompt`'s user message joins the conversation.
//!
//! ACP v1 answers a prompt when its turn ends, so nothing is reported early.
//! ACP v2 answers once the message is inserted, with the message's ID, and
//! requires a replay to reuse that ID. A model turn therefore saves the user
//! message before its run starts and reports the ID replay derives from the
//! saved transcript; a command handled without a turn is reported with a
//! fresh ID, since nothing of it is replayed.

use agent_client_protocol::schema::v1::MessageId;

/// Reports a prompt's user message, once, when the session inserts it.
pub(super) enum PromptAcceptance {
    /// ACP v1: the prompt response reports the turn.
    Implicit,
    /// ACP v2: called with the inserted message's ID before the prompt sends
    /// any of its turn's updates.
    Report(Box<dyn FnOnce(MessageId) + Send + Sync>),
}

impl PromptAcceptance {
    /// Whether the session must name the inserted message.
    pub(super) fn wants_id(&self) -> bool {
        matches!(self, Self::Report(_))
    }

    /// Reports `id`; later calls do nothing.
    pub(super) fn accept(&mut self, id: impl FnOnce() -> MessageId) {
        if let Self::Report(report) = std::mem::replace(self, Self::Implicit) {
            report(id());
        }
    }

    /// Reports a message that is not replayed, such as a local command.
    pub(super) fn accept_unreplayed(&mut self) {
        self.accept(|| MessageId::new(format!("rho-user-{}", uuid::Uuid::new_v4())));
    }
}
