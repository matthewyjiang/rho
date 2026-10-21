use serde_json::json;

use crate::model::{
    AssistantMessage, ContextUsage, ModelIdentity, ModelUsage, ToolCall, ToolResult,
};

use super::*;

// Covers: the context estimate counts every part a request carries (text,
// tool calls and results, provider replay context, tool schemas), and message
// slices omit request overhead.
// Owner: SDK context estimation.
#[test]
fn estimates_include_every_request_part() {
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
    let identity = ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
    let context = ProviderContextBlock {
        identity: identity.clone(),
        kind: "openai_response_output_item".into(),
        position: Some(0),
        data: json!({"type": "reasoning", "encrypted_content": "x".repeat(400)}),
    };
    let replay = Message::assistant(AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(identity),
        reasoning_summary: None,
        provider_context: vec![context.clone()],
    });
    let spec = ToolSpec {
        name: "read_file".into(),
        description: "read a file".into(),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    };
    let cases = [
        (
            "text messages",
            estimate_context_tokens(
                &[
                    Message::System("12345678".into()),
                    Message::user_text("123456789"),
                ],
                &[],
            ),
            REQUEST_OVERHEAD_TOKENS
                + MESSAGE_OVERHEAD_TOKENS
                + 2
                + MESSAGE_OVERHEAD_TOKENS
                + CONTENT_BLOCK_OVERHEAD_TOKENS
                + 3,
        ),
        (
            "tool calls and results",
            estimate_context_tokens(
                &[
                    Message::Assistant(vec![ContentBlock::ToolCall(call.clone())]),
                    Message::ToolResult(result.clone()),
                ],
                &[],
            ),
            REQUEST_OVERHEAD_TOKENS
                + MESSAGE_OVERHEAD_TOKENS
                + CONTENT_BLOCK_OVERHEAD_TOKENS
                + TOOL_CALL_OVERHEAD_TOKENS
                + json_tokens(&call)
                + TOOL_RESULT_OVERHEAD_TOKENS
                + json_tokens(&result),
        ),
        (
            "provider replay context",
            estimate_message_tokens(&replay),
            MESSAGE_OVERHEAD_TOKENS
                + CONTENT_BLOCK_OVERHEAD_TOKENS
                + text_tokens("answer")
                + json_tokens(&context.data),
        ),
        (
            "tool schemas",
            estimate_context_tokens(&[], std::slice::from_ref(&spec)),
            REQUEST_OVERHEAD_TOKENS + TOOL_SCHEMA_OVERHEAD_TOKENS + json_tokens(&spec),
        ),
        (
            "message slices without request overhead",
            estimate_messages_tokens(&[
                Message::user_text("1234"),
                Message::assistant_text("12345678"),
            ]),
            MESSAGE_OVERHEAD_TOKENS
                + CONTENT_BLOCK_OVERHEAD_TOKENS
                + 1
                + MESSAGE_OVERHEAD_TOKENS
                + CONTENT_BLOCK_OVERHEAD_TOKENS
                + 2,
        ),
    ];

    for (case, actual, expected) in cases {
        assert_eq!(actual, expected, "{case}");
    }
}

// Covers: provider usage becomes current context from the full input total,
// or from total minus output when only a mixed total is reported.
// Owner: SDK context estimation.
#[test]
fn provider_usage_becomes_current_context_from_total_input() {
    let cases = [
        (
            "input plus cache reads and writes",
            ModelUsage {
                input_tokens: Some(300),
                cache_read_tokens: Some(700),
                cache_write_tokens: Some(2_000),
                context_window: Some(10_000),
                ..ModelUsage::default()
            },
            3_000,
        ),
        (
            "mixed prompt total minus output",
            ModelUsage {
                output_tokens: Some(5),
                total_tokens: Some(105),
                context_window: Some(10_000),
                ..ModelUsage::default()
            },
            100,
        ),
    ];

    for (case, usage, expected) in cases {
        assert_eq!(
            ContextUsage::from_model_usage(&usage),
            Some(ContextUsage::provider_reported(expected, Some(10_000))),
            "{case}"
        );
    }
}
