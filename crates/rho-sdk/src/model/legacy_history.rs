//! Read-side conversion for history encodings saved by rho-sdk 5.x.
//!
//! Before history had typed entries, 5.x stored compaction summaries and model
//! context as labelled user-role text, async tool calls as marker blocks in
//! assistant provider context, and provider notes and Codex cross-model
//! formats as reserved kinds. Deserialization recognizes those encodings
//! exactly and converts them one-for-one, so every loader (snapshots, host
//! transcripts) sees typed history without shifting indexes. Unknown shapes
//! are left untouched, and converting typed history is a no-op.
//!
//! Message recognition is not gated on a schema version, because it runs inside
//! serde where none is available. A 6.0 user message whose text is exactly a
//! 5.x summary or model-context encoding is therefore reread as that entry.
//! That only removes authority from the message, never grants it.

use std::sync::Arc;

use serde_json::Value;

use super::{
    model_context, CompactionSummary, ContentBlock, Message, ProviderContextBlock, ReplayScope,
};
use crate::tool::ToolExecutionMode;

const ASYNC_TOOL_CALL_KIND: &str = "rho.sdk.async_tool_call.v1";
const PROVIDER_NOTE_KIND_PREFIX: &str = "rho.sdk.provider_note.v1:";

/// Converts one 5.x history entry to its typed form.
pub(super) fn upgrade_message(message: Message) -> Message {
    match message {
        Message::User(content) => {
            if let Some(summary) = CompactionSummary::from_legacy_user_content(&content) {
                Message::CompactionSummary(summary)
            } else if let Some(text) = model_context::from_legacy_user_content(&content) {
                Message::ModelContext(text)
            } else {
                Message::User(content)
            }
        }
        Message::EnrichedAssistant(mut message) => {
            let message_mut = Arc::make_mut(&mut message);
            attach_async_markers(&mut message_mut.content, &mut message_mut.provider_context);
            Message::EnrichedAssistant(message)
        }
        Message::AbortedAssistant(mut message) => {
            let message_mut = Arc::make_mut(&mut message);
            attach_async_markers(&mut message_mut.content, &mut message_mut.provider_context);
            Message::AbortedAssistant(message)
        }
        message @ (Message::System(_)
        | Message::Assistant(_)
        | Message::ToolResult(_)
        | Message::CompactionSummary(_)
        | Message::ModelContext(_)
        | Message::BoundaryInput(_)
        | Message::ReasoningShift(_)) => message,
    }
}

/// Moves 5.x async-call marker blocks onto [`super::ToolCall::execution`].
fn attach_async_markers(content: &mut [ContentBlock], context: &mut Vec<ProviderContextBlock>) {
    context.retain(|block| {
        let Some(call_id) = legacy_async_call_id(block) else {
            return true;
        };
        for block in content.iter_mut() {
            if let ContentBlock::ToolCall(call) = block {
                if call.id == call_id {
                    call.execution = ToolExecutionMode::Async;
                }
            }
        }
        false
    });
}

fn legacy_async_call_id(block: &ProviderContextBlock) -> Option<&str> {
    (block.kind == ASYNC_TOOL_CALL_KIND && block.position.is_none())
        .then(|| block.data.as_str())
        .flatten()
}

/// Derives the replay scope of a block saved without one.
pub(super) fn upgrade_provider_context(block: &mut ProviderContextBlock) {
    if let Some(name) = block
        .kind
        .strip_prefix(PROVIDER_NOTE_KIND_PREFIX)
        .filter(|name| !name.is_empty())
    {
        block.kind = name.to_owned();
        block.replay = ReplayScope::Never;
    } else if is_codex_cross_model_format(block) {
        block.replay = ReplayScope::ProviderApi;
    }
}

/// Codex Responses compaction items and reasoning items with nonempty
/// encrypted content, which 5.x replayed across Codex model names.
fn is_codex_cross_model_format(block: &ProviderContextBlock) -> bool {
    if (
        block.identity.provider.as_str(),
        block.identity.api.as_str(),
        block.kind.as_str(),
    ) != (
        "openai-codex",
        "openai-responses",
        "openai_response_output_item",
    ) {
        return false;
    }
    match block.data.get("type").and_then(Value::as_str) {
        Some("compaction") => true,
        Some("reasoning") => block
            .data
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|content| !content.is_empty()),
        _ => false,
    }
}

#[cfg(test)]
#[path = "legacy_history_tests.rs"]
mod tests;
