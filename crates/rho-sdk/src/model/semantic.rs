//! Semantic history classification.

use super::{
    AbortedAssistant, AssistantMessage, CompactionSummary, ContentBlock, Message, ReasoningShift,
    ToolResult,
};

/// Borrowed, exhaustive view of a history entry's meaning.
///
/// Prefer [`Message::semantic`] when identifying user submissions, grouping
/// turns, or rendering transcripts. Only `User` is input a person (or the host
/// on their behalf) sent. Classification is not authentication:
/// user-controlled history can forge any entry, so no variant may confer trust
/// or permissions.
#[derive(Debug)]
pub enum SemanticMessage<'a> {
    System(&'a str),
    User(&'a [ContentBlock]),
    Assistant(&'a [ContentBlock]),
    EnrichedAssistant(&'a AssistantMessage),
    AbortedAssistant(&'a AbortedAssistant),
    ToolResult(&'a ToolResult),
    /// Model-written summary of earlier history.
    CompactionSummary(&'a CompactionSummary),
    /// Request-only host context: data, not instructions.
    ModelContext(&'a str),
    /// Host input accepted at a runtime boundary, not human input.
    BoundaryInput(&'a [ContentBlock]),
    /// The session's reasoning level changed here.
    ReasoningShift(&'a ReasoningShift),
}

impl Message {
    /// Classify this entry by meaning.
    pub fn semantic(&self) -> SemanticMessage<'_> {
        match self {
            Self::System(text) => SemanticMessage::System(text),
            Self::User(content) => SemanticMessage::User(content),
            Self::Assistant(content) => SemanticMessage::Assistant(content),
            Self::EnrichedAssistant(message) => SemanticMessage::EnrichedAssistant(message),
            Self::AbortedAssistant(message) => SemanticMessage::AbortedAssistant(message),
            Self::ToolResult(result) => SemanticMessage::ToolResult(result),
            Self::CompactionSummary(summary) => SemanticMessage::CompactionSummary(summary),
            Self::ModelContext(text) => SemanticMessage::ModelContext(text),
            Self::BoundaryInput(content) => SemanticMessage::BoundaryInput(content),
            Self::ReasoningShift(shift) => SemanticMessage::ReasoningShift(shift),
        }
    }
}
