use serde::Serialize;

use super::{ContentBlock, Message, ProviderContextBlock, ToolSpec};

const REQUEST_OVERHEAD_TOKENS: u64 = 3;
const MESSAGE_OVERHEAD_TOKENS: u64 = 4;
const CONTENT_BLOCK_OVERHEAD_TOKENS: u64 = 1;
const TOOL_CALL_OVERHEAD_TOKENS: u64 = 8;
const TOOL_RESULT_OVERHEAD_TOKENS: u64 = 6;
const TOOL_SCHEMA_OVERHEAD_TOKENS: u64 = 12;
const CHARS_PER_TOKEN: u64 = 4;

pub fn estimate_context_tokens(messages: &[Message], tools: &[ToolSpec]) -> u64 {
    REQUEST_OVERHEAD_TOKENS
        .saturating_add(estimate_messages_tokens(messages))
        .saturating_add(tools.iter().map(tool_spec_tokens).sum::<u64>())
}

pub fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message_tokens).sum()
}

pub fn estimate_message_tokens(message: &Message) -> u64 {
    match message {
        Message::System(text) => MESSAGE_OVERHEAD_TOKENS.saturating_add(text_tokens(text)),
        Message::User(blocks) | Message::Assistant(blocks) => MESSAGE_OVERHEAD_TOKENS
            .saturating_add(blocks.iter().map(content_block_tokens).sum::<u64>()),
        Message::EnrichedAssistant(message) => assistant_message_tokens(
            &message.content,
            message.reasoning_summary.as_deref(),
            &message.provider_context,
        ),
        Message::AbortedAssistant(message) => assistant_message_tokens(
            &message.content,
            message.reasoning_summary.as_deref(),
            &message.provider_context,
        ),
        Message::ToolResult(result) => {
            TOOL_RESULT_OVERHEAD_TOKENS.saturating_add(json_tokens(result))
        }
    }
}

fn assistant_message_tokens(
    content: &[ContentBlock],
    reasoning_summary: Option<&str>,
    provider_context: &[ProviderContextBlock],
) -> u64 {
    let summary_tokens = reasoning_summary.map(text_tokens).unwrap_or_default();
    let replay_tokens = provider_context
        .iter()
        .map(|block| json_tokens(&block.data))
        .sum::<u64>();
    MESSAGE_OVERHEAD_TOKENS
        .saturating_add(content.iter().map(content_block_tokens).sum::<u64>())
        .saturating_add(summary_tokens.max(replay_tokens))
}

fn content_block_tokens(block: &ContentBlock) -> u64 {
    CONTENT_BLOCK_OVERHEAD_TOKENS.saturating_add(match block {
        ContentBlock::Text(text) => text_tokens(text),
        ContentBlock::Image(image) => {
            85_u64.saturating_add((image.data.len() as u64).div_ceil(4096))
        }
        ContentBlock::ToolCall(call) => TOOL_CALL_OVERHEAD_TOKENS.saturating_add(json_tokens(call)),
    })
}

fn tool_spec_tokens(spec: &ToolSpec) -> u64 {
    TOOL_SCHEMA_OVERHEAD_TOKENS.saturating_add(json_tokens(spec))
}

fn text_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(CHARS_PER_TOKEN)
}

fn json_tokens(value: &impl Serialize) -> u64 {
    serde_json::to_string(value)
        .map(|json| text_tokens(&json))
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
