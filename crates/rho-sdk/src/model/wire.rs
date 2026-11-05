//! Provider-facing roles for history entries.
//!
//! Provider converters match [`WireMessage`] instead of [`Message`], so SDK
//! history entries without a native provider form (compaction summaries,
//! model context, boundary input) reach every provider as identical
//! user-role content. Exhaustive like [`Message`]: an entry that needs a new
//! wire role is a breaking change every converter must handle.

use std::borrow::Cow;

use super::{
    model_context, AbortedAssistant, AssistantMessage, ContentBlock, Message, ReasoningShift,
    ToolResult,
};

/// How a provider sends one history entry.
#[derive(Debug)]
pub enum WireMessage<'a> {
    System(&'a str),
    /// User-role content: human input, or the labelled rendering of an SDK
    /// entry such as a compaction summary. Classify by [`Message::semantic`],
    /// not by this role.
    User(Cow<'a, [ContentBlock]>),
    Assistant(&'a [ContentBlock]),
    EnrichedAssistant(&'a AssistantMessage),
    AbortedAssistant(&'a AbortedAssistant),
    ToolResult(&'a ToolResult),
    /// Sends nothing unless the provider pins reasoning effort mid-conversation.
    ReasoningShift(&'a ReasoningShift),
}

impl Message {
    /// The provider-facing role and content of this entry.
    pub fn wire(&self) -> WireMessage<'_> {
        match self {
            Self::System(text) => WireMessage::System(text),
            Self::User(content) | Self::BoundaryInput(content) => {
                WireMessage::User(Cow::Borrowed(content))
            }
            Self::CompactionSummary(summary) => {
                WireMessage::User(Cow::Owned(summary.user_role_content()))
            }
            Self::ModelContext(text) => {
                WireMessage::User(Cow::Owned(model_context::user_role_content(text)))
            }
            Self::Assistant(content) => WireMessage::Assistant(content),
            Self::EnrichedAssistant(message) => WireMessage::EnrichedAssistant(message),
            Self::AbortedAssistant(message) => WireMessage::AbortedAssistant(message),
            Self::ToolResult(result) => WireMessage::ToolResult(result),
            Self::ReasoningShift(shift) => WireMessage::ReasoningShift(shift),
        }
    }
}
