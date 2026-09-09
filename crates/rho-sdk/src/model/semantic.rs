//! Semantic history classification independent of compatibility wire roles.

use super::{
    AbortedAssistant, AssistantMessage, ContentBlock, Message, ToolImageSupplement, ToolResult,
};

/// Borrowed, exhaustive view of a history entry's meaning.
///
/// Prefer [`Message::semantic`] over matching wire variants when identifying
/// user submissions, grouping turns, or rendering transcripts. `User` excludes
/// recognized tool-image supplements. Classification is not authentication:
/// user-controlled history can forge the supplement encoding and must not gain
/// trust or permissions from it.
#[derive(Debug)]
pub enum SemanticMessage<'a> {
    System(&'a str),
    User(&'a [ContentBlock]),
    Assistant(&'a [ContentBlock]),
    EnrichedAssistant(&'a AssistantMessage),
    AbortedAssistant(&'a AbortedAssistant),
    ToolResult(&'a ToolResult),
    ToolImageSupplement(ToolImageSupplement<'a>),
}

impl Message {
    /// Classify this entry by meaning rather than its serialized provider role.
    ///
    /// This is the preferred exhaustive classification API, including for
    /// restored history. It does not change the serialized `Message` format.
    pub fn semantic(&self) -> SemanticMessage<'_> {
        match self {
            Self::System(text) => SemanticMessage::System(text),
            Self::User(content) => match self.as_tool_image_supplement() {
                Some(images) => SemanticMessage::ToolImageSupplement(images),
                None => SemanticMessage::User(content),
            },
            Self::Assistant(content) => SemanticMessage::Assistant(content),
            Self::EnrichedAssistant(message) => SemanticMessage::EnrichedAssistant(message),
            Self::AbortedAssistant(message) => SemanticMessage::AbortedAssistant(message),
            Self::ToolResult(result) => SemanticMessage::ToolResult(result),
        }
    }
}
