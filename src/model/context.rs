use serde::{Deserialize, Serialize};

use crate::tool::ToolSpec;

use super::{ContentBlock, Message, ModelUsage};

const REQUEST_OVERHEAD_TOKENS: u64 = 3;
const MESSAGE_OVERHEAD_TOKENS: u64 = 4;
const CONTENT_BLOCK_OVERHEAD_TOKENS: u64 = 1;
const TOOL_CALL_OVERHEAD_TOKENS: u64 = 8;
const TOOL_RESULT_OVERHEAD_TOKENS: u64 = 6;
const TOOL_SCHEMA_OVERHEAD_TOKENS: u64 = 12;
const CHARS_PER_TOKEN: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextUsageSource {
    Estimated,
    ProviderReported,
    UnknownAfterCompaction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub source: ContextUsageSource,
}

impl ContextUsage {
    pub fn estimated(tokens: u64, context_window: Option<u64>) -> Self {
        Self {
            tokens: Some(tokens),
            context_window,
            source: ContextUsageSource::Estimated,
        }
    }

    pub fn provider_reported(tokens: u64, context_window: Option<u64>) -> Self {
        Self {
            tokens: Some(tokens),
            context_window,
            source: ContextUsageSource::ProviderReported,
        }
    }

    pub fn unknown_after_compaction(context_window: Option<u64>) -> Self {
        Self {
            tokens: None,
            context_window,
            source: ContextUsageSource::UnknownAfterCompaction,
        }
    }

    pub fn from_model_usage(usage: &ModelUsage) -> Option<Self> {
        usage
            .total_input_tokens()
            .map(|tokens| Self::provider_reported(tokens, usage.context_window))
    }
}

pub fn estimate_context_usage(
    messages: &[Message],
    tools: &[ToolSpec],
    context_window: Option<u64>,
) -> ContextUsage {
    ContextUsage::estimated(estimate_context_tokens(messages, tools), context_window)
}

pub fn estimate_context_tokens(messages: &[Message], tools: &[ToolSpec]) -> u64 {
    REQUEST_OVERHEAD_TOKENS
        .saturating_add(messages.iter().map(message_tokens).sum::<u64>())
        .saturating_add(tools.iter().map(tool_spec_tokens).sum::<u64>())
}

fn message_tokens(message: &Message) -> u64 {
    match message {
        Message::System(text) => MESSAGE_OVERHEAD_TOKENS.saturating_add(text_tokens(text)),
        Message::User(blocks) | Message::Assistant(blocks) => MESSAGE_OVERHEAD_TOKENS
            .saturating_add(blocks.iter().map(content_block_tokens).sum::<u64>()),
        Message::ToolResult(result) => {
            TOOL_RESULT_OVERHEAD_TOKENS.saturating_add(json_tokens(result))
        }
    }
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
mod tests {
    use serde_json::json;

    use crate::tool::{ToolCall, ToolResult};

    use super::*;

    #[test]
    fn estimates_text_messages_with_overhead() {
        let messages = vec![
            Message::System("12345678".into()),
            Message::user_text("123456789"),
        ];

        assert_eq!(
            estimate_context_tokens(&messages, &[]),
            REQUEST_OVERHEAD_TOKENS
                + MESSAGE_OVERHEAD_TOKENS
                + 2
                + MESSAGE_OVERHEAD_TOKENS
                + CONTENT_BLOCK_OVERHEAD_TOKENS
                + 3
        );
    }

    #[test]
    fn includes_tool_calls_and_tool_results() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: json!({"path": "src/main.rs"}),
        };
        let result = ToolResult {
            id: "call_1".into(),
            ok: true,
            content: "file contents".into(),
        };
        let messages = vec![
            Message::Assistant(vec![ContentBlock::ToolCall(call.clone())]),
            Message::ToolResult(result.clone()),
        ];

        assert_eq!(
            estimate_context_tokens(&messages, &[]),
            REQUEST_OVERHEAD_TOKENS
                + MESSAGE_OVERHEAD_TOKENS
                + CONTENT_BLOCK_OVERHEAD_TOKENS
                + TOOL_CALL_OVERHEAD_TOKENS
                + json_tokens(&call)
                + TOOL_RESULT_OVERHEAD_TOKENS
                + json_tokens(&result)
        );
    }

    #[test]
    fn includes_tool_schema_tokens() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };

        assert_eq!(
            estimate_context_tokens(&[], std::slice::from_ref(&spec)),
            REQUEST_OVERHEAD_TOKENS + TOOL_SCHEMA_OVERHEAD_TOKENS + json_tokens(&spec)
        );
    }

    #[test]
    fn provider_usage_becomes_current_context_from_total_input() {
        let usage = ModelUsage {
            input_tokens: Some(300),
            cache_read_tokens: Some(700),
            cache_write_tokens: Some(2_000),
            context_window: Some(10_000),
            ..ModelUsage::default()
        };

        assert_eq!(
            ContextUsage::from_model_usage(&usage),
            Some(ContextUsage::provider_reported(3_000, Some(10_000)))
        );
    }
}
