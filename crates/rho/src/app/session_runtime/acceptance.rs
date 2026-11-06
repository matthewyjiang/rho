//! When a prompt's user message joins the conversation.
//!
//! Some hosts answer a prompt only when its turn ends, so nothing is reported
//! early. Others (ACP v2) answer once the message is inserted, naming the
//! message, and require a replay to reuse that name. A prompt whose message
//! is saved (a model turn, before its run starts, or a workflow start) is
//! accepted as the save lands, with the display row a replay shows it as;
//! see [`super::SessionRuntime::save_prompt_rows`]. A command that saves
//! nothing, such as `/compact`, is accepted as unreplayed.

use crate::session::tree::DisplayRowOrigin;

/// The user message a prompt inserted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InsertedMessage {
    /// Saved as this display row; a replay shows the message from it.
    Saved(DisplayRowOrigin),
    /// Not replayed, such as the prompt of a local command.
    Unreplayed,
}

/// Reports a prompt's user message, once, when the session inserts it.
pub(crate) enum MessageAcceptance {
    /// The host learns of the message from the turn's end.
    Implicit,
    /// Called with the inserted message before the prompt reports any of
    /// its turn's events.
    Report(Box<dyn FnOnce(InsertedMessage) + Send + Sync>),
}

impl MessageAcceptance {
    /// Whether the session must name the inserted message.
    pub(super) fn wants_id(&self) -> bool {
        matches!(self, Self::Report(_))
    }

    /// Reports `message`; later calls do nothing.
    pub(super) fn accept(&mut self, message: InsertedMessage) {
        if let Self::Report(report) = std::mem::replace(self, Self::Implicit) {
            report(message);
        }
    }

    /// Reports a message that is not replayed, such as a local command.
    pub(crate) fn accept_unreplayed(&mut self) {
        if self.wants_id() {
            self.accept(InsertedMessage::Unreplayed);
        }
    }
}
