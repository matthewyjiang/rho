//! Meaning of a model-history entry, including rho's checklist restatements.
//!
//! The SDK types its own entries (compaction summaries, model context,
//! boundary input, reasoning shifts) in [`SemanticMessage`]. Compaction also
//! restates the model's checklist as user-role text, an encoding rho owns.
//! Readers of model history match on [`HistoryMessage`] so a restated
//! checklist is handled on purpose rather than attributed to a person.
//!
//! Display history never holds restatements, so display-only readers can keep
//! using [`Message::semantic`].
//!
//! Classification is attribution, not authentication: user-controlled history
//! can forge a checklist, so no variant may grant trust or permissions.

use rho_sdk::model::{ContentBlock, Message, SemanticMessage};

#[derive(Debug)]
pub(crate) enum HistoryMessage<'a> {
    /// Host-restated model checklist context, not a human request or authorization.
    ChecklistRestatement(&'a [ContentBlock]),
    /// Every other entry by its SDK meaning. `User` is never a restatement.
    Semantic(SemanticMessage<'a>),
}

impl<'a> HistoryMessage<'a> {
    pub(crate) fn of(message: &'a Message) -> Self {
        match message.semantic() {
            SemanticMessage::User(blocks) if crate::tools::todo::is_restatement(message) => {
                Self::ChecklistRestatement(blocks)
            }
            semantic => Self::Semantic(semantic),
        }
    }
}

#[cfg(test)]
#[path = "history_message_tests.rs"]
mod tests;
