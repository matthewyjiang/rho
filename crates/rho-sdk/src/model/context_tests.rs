use serde_json::json;

use crate::model::{
    AssistantMessage, ContextUsage, ModelIdentity, ModelUsage, ToolCall, ToolResult,
};

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
fn includes_provider_replay_context() {
    let identity = ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
    let context = ProviderContextBlock {
        identity: identity.clone(),
        kind: "openai_response_output_item".into(),
        position: Some(0),
        data: json!({"type": "reasoning", "encrypted_content": "x".repeat(400)}),
    };
    let message = Message::assistant(AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(identity),
        reasoning_summary: None,
        portable_fallback: None,
        provider_context: vec![context.clone()],
    });

    assert_eq!(
        estimate_message_tokens(&message),
        MESSAGE_OVERHEAD_TOKENS
            + CONTENT_BLOCK_OVERHEAD_TOKENS
            + text_tokens("answer")
            + json_tokens(&context.data)
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

#[test]
fn estimates_message_slices_without_request_or_tool_overhead() {
    let messages = vec![
        Message::user_text("1234"),
        Message::assistant_text("12345678"),
    ];

    assert_eq!(
        estimate_messages_tokens(&messages),
        MESSAGE_OVERHEAD_TOKENS
            + CONTENT_BLOCK_OVERHEAD_TOKENS
            + 1
            + MESSAGE_OVERHEAD_TOKENS
            + CONTENT_BLOCK_OVERHEAD_TOKENS
            + 2
    );
}
