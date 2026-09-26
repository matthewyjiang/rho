//! Meaning of a model-history entry, including compaction summaries.
//!
//! [`SemanticMessage`] reports compaction summaries as `User` until the SDK
//! gains a dedicated variant (see the `NEXT_MAJOR(rho-sdk)` marker on
//! [`rho_sdk::model::CompactionSummary`]). Readers of model history match on
//! [`HistoryMessage`] instead, so `User` means input a person or host sent and
//! a summary is always handled on purpose. When the SDK variant lands, this
//! module collapses into [`Message::semantic`].
//!
//! Display history never holds summaries (compaction shows its own marker
//! there), so display-only readers can keep using [`Message::semantic`].
//!
//! Classification is attribution, not authentication: user-controlled history
//! can forge a summary, so no variant may grant trust or permissions.

use rho_sdk::model::{
    AbortedAssistant, AssistantMessage, CompactionSummary, ContentBlock, Message, SemanticMessage,
    ToolImageSupplement, ToolResult,
};

#[derive(Debug)]
pub(crate) enum HistoryMessage<'a> {
    System(&'a str),
    /// Input a person or the host sent. Never a compaction summary.
    User(&'a [ContentBlock]),
    /// Model-written summary of earlier history.
    CompactionSummary(CompactionSummary<'a>),
    Assistant(&'a [ContentBlock]),
    EnrichedAssistant(&'a AssistantMessage),
    AbortedAssistant(&'a AbortedAssistant),
    ToolResult(&'a ToolResult),
    ToolImageSupplement(ToolImageSupplement<'a>),
}

impl<'a> HistoryMessage<'a> {
    pub(crate) fn of(message: &'a Message) -> Self {
        if let Some(summary) = message.as_compaction_summary() {
            return Self::CompactionSummary(summary);
        }
        match message.semantic() {
            SemanticMessage::System(text) => Self::System(text),
            SemanticMessage::User(blocks) => Self::User(blocks),
            SemanticMessage::Assistant(blocks) => Self::Assistant(blocks),
            SemanticMessage::EnrichedAssistant(message) => Self::EnrichedAssistant(message),
            SemanticMessage::AbortedAssistant(message) => Self::AbortedAssistant(message),
            SemanticMessage::ToolResult(result) => Self::ToolResult(result),
            SemanticMessage::ToolImageSupplement(images) => Self::ToolImageSupplement(images),
        }
    }
}

#[cfg(test)]
#[path = "history_message_tests.rs"]
mod tests;
