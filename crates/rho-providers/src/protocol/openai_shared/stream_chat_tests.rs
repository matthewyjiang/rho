use crate::model::{ContentBlock, ModelEvent, ModelResponse, ToolCall};
use crate::protocol::openai_chat::{ChatStreamAccumulator, ChatToolCallPolicy};
use crate::protocol::openai_shared::usage::HiddenReasoningRisk;
use pretty_assertions::assert_eq;
use serde_json::json;

// Covers: reasoning_content deltas must accumulate and emit provider context
// Owner: openai chat completions streaming
#[test]
fn chat_stream_emits_reasoning_content_provider_context() {
    let mut chat_stream = ChatStreamAccumulator::default();
    let mut events = Vec::new();
    let mut on_event = |event: ModelEvent| {
        events.push(event);
        Ok(())
    };

    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"plan "}}]}"#,
            &mut on_event,
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"next"}}]}"#,
            &mut on_event,
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"pwd\"}"}}]}}]}"#,
            &mut on_event,
        )
        .unwrap();

    let response = chat_stream.finish(&mut on_event).unwrap();

    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(
                blocks.as_slice(),
                [ContentBlock::ToolCall(call)] if call.id == "call-1" && call.name == "bash"
            )
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::ReasoningDelta(text) if text == "plan "
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::ProviderContext {
            kind,
            position: Some(0),
            data,
        } if kind == "openai_chat_reasoning_content"
            && data.as_str() == Some("plan next")
    )));
}

// Covers: streamed tool-call argument fragments must assemble into one call
// Owner: openai chat completions streaming
#[test]
fn accumulates_streamed_tool_call_deltas() {
    let mut chat_stream = ChatStreamAccumulator::default();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"bash","arguments":"{\"command\":"}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"pwd\"}"}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();

    let response = chat_stream.finish(&mut |_| Ok(())).unwrap();
    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: json!({"command": "pwd"}),
        })])
    );
}

// Covers: reasoning that only appears in the final message snapshot never
// streamed, so itemized reasoning tokens must still subtract from the
// throughput numerator instead of reporting the inflated aggregate total
// Owner: openai chat completions streaming
#[test]
fn final_snapshot_reasoning_does_not_count_as_streamed_for_throughput() {
    let mut chat_stream =
        ChatStreamAccumulator::new(ChatToolCallPolicy::Strict, HiddenReasoningRisk::Possible);
    let mut events = Vec::new();
    let mut on_event = |event: ModelEvent| {
        events.push(event);
        Ok(())
    };

    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"content":"answer"}}]}"#,
            &mut on_event,
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"usage":{"prompt_tokens":10,"completion_tokens":30,"completion_tokens_details":{"reasoning_tokens":12}},"choices":[{"delta":{},"finish_reason":"stop","message":{"role":"assistant","content":"answer","reasoning_content":"hidden chain"}}]}"#,
            &mut on_event,
        )
        .unwrap();

    let response = chat_stream.finish(&mut on_event).unwrap();

    assert!(matches!(
        response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "answer")
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::ProviderContext { kind, data, .. }
            if kind == "rho_model_call_generation_output_tokens"
                && data["tokens"] == json!(18)
    )));
}

// Covers: a final usage snapshot without cache details must not combine its
// raw input total with cache buckets retained from an earlier snapshot
// Owner: openai chat completions streaming
#[test]
fn partial_final_usage_snapshot_keeps_input_buckets_disjoint() {
    let mut chat_stream = ChatStreamAccumulator::default();
    let mut events = Vec::new();
    let mut on_event = |event: ModelEvent| {
        events.push(event);
        Ok(())
    };

    chat_stream
        .handle_line(
            r#"data: {"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":50},"completion_tokens":1},"choices":[{"delta":{"content":"a"}}]}"#,
            &mut on_event,
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"usage":{"prompt_tokens":100,"completion_tokens":2},"choices":[]}"#,
            &mut on_event,
        )
        .unwrap();

    chat_stream.finish(&mut on_event).unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::Usage(usage)
            if usage.input_tokens == Some(50)
                && usage.cache_read_tokens == Some(50)
                && usage.output_tokens == Some(2)
    )));
}
// Owner: openai chat completions streaming
#[test]
fn streamed_tool_calls_tolerate_qwen_style_quirks() {
    let mut chat_stream =
        ChatStreamAccumulator::new(ChatToolCallPolicy::Lenient, HiddenReasoningRisk::Unlikely);
    // index 1 first leaves a hole at 0; arguments arrive as a JSON object value.
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"dup","type":"function","function":{"name":"bash","arguments":{"command":"pwd"}}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":2,"id":"dup","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.rs\"}"}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();

    let response = chat_stream.finish(&mut |_| Ok(())).unwrap();
    assert_eq!(
        response,
        ModelResponse::Assistant(vec![
            ContentBlock::ToolCall(ToolCall {
                id: "dup".into(),
                name: "bash".into(),
                arguments: json!({"command": "pwd"}),
            }),
            ContentBlock::ToolCall(ToolCall {
                id: "dup_2".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
            }),
        ])
    );
}

// Covers: default strict policy must not invent ids for sparse empty slots
// Owner: openai chat completions tool-call normalization
#[test]
fn streamed_tool_calls_strict_policy_rejects_sparse_empty_slots() {
    let mut chat_stream = ChatStreamAccumulator::default();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call-1","type":"function","function":{"name":"bash","arguments":"{}"}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();
    let err = chat_stream.finish(&mut |_| Ok(())).unwrap_err();
    assert!(matches!(
        err,
        crate::model::ModelError::InvalidResponse(message)
            if message == "tool call 0 missing id"
    ));
}

// Covers: non-object argument JSON must fail loud, not become invented parameters
// Owner: openai chat completions tool-call normalization
#[test]
fn chat_tool_calls_reject_non_object_arguments() {
    let mut chat_stream = ChatStreamAccumulator::default();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"bash","arguments":"42"}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();

    let err = chat_stream.finish(&mut |_| Ok(())).unwrap_err();
    assert!(matches!(
        err,
        crate::model::ModelError::InvalidResponse(message)
            if message == "tool call arguments for bash are not a JSON object"
    ));
}

// Covers: final message snapshot can complete tool calls missing from deltas
// Owner: openai chat completions streaming
#[test]
fn final_message_snapshot_fills_incomplete_streamed_tool_calls() {
    let mut chat_stream = ChatStreamAccumulator::default();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-9","function":{"name":"bash"}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","message":{"role":"assistant","content":"","tool_calls":[{"id":"call-9","type":"function","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]}}]}"#,
            &mut |_| Ok(()),
        )
        .unwrap();

    let response = chat_stream.finish(&mut |_| Ok(())).unwrap();
    let ModelResponse::Assistant(blocks) = response;
    assert!(matches!(
        blocks.as_slice(),
        [ContentBlock::ToolCall(call)]
            if call.id == "call-9"
                && call.name == "bash"
                && call.arguments == json!({"command":"ls"})
                && call.arguments.is_object()
                && !call.id.is_empty()
    ));
}
