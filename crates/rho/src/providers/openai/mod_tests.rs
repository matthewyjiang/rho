use super::*;
use crate::model::{AbortedAssistant, ContentBlock, ImageContent, Message, PartialToolCall};
use crate::protocol::openai_chat::{
    convert_streamed_response, handle_openai_stream_line, to_openai_message_for_target,
};
use crate::protocol::openai_responses::{
    codex_input_items, codex_reasoning_param, extract_sse_text, handle_codex_sse_line,
    CodexSseState,
};
use crate::reasoning::ReasoningLevel;
use crate::tool::{ToolCall, ToolResult, ToolSpec};
use serde_json::json;

#[test]
fn codex_reasoning_param_preserves_none_effort() {
    assert_eq!(
        codex_reasoning_param(Some("none"), None).unwrap(),
        json!({"effort":"none"})
    );
    assert!(codex_reasoning_param(None, Some("none")).is_none());
    assert_eq!(
        codex_reasoning_param(Some("low"), Some("auto")).unwrap(),
        json!({"effort":"low","summary":"auto"})
    );
}

#[test]
fn reasoning_level_maps_to_codex_reasoning_param() {
    assert!(codex_reasoning_param(
        crate::reasoning::ReasoningLevel::Off.effort(),
        crate::reasoning::ReasoningLevel::Off.summary()
    )
    .is_none());
    assert_eq!(
        codex_reasoning_param(
            crate::reasoning::ReasoningLevel::Minimal.effort(),
            crate::reasoning::ReasoningLevel::Minimal.summary()
        )
        .unwrap(),
        json!({"effort":"minimal","summary":"auto"})
    );
    assert_eq!(
        codex_reasoning_param(
            crate::reasoning::ReasoningLevel::Xhigh.effort(),
            crate::reasoning::ReasoningLevel::Xhigh.summary()
        )
        .unwrap(),
        json!({"effort":"xhigh","summary":"auto"})
    );
    assert_eq!(
        codex_reasoning_param(
            crate::reasoning::ReasoningLevel::Max.effort(),
            crate::reasoning::ReasoningLevel::Max.summary()
        )
        .unwrap(),
        json!({"effort":"max","summary":"auto"})
    );
}

#[test]
fn openai_reasoning_normalization_never_turns_requested_reasoning_off() {
    let supported = [
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::High,
    ];
    assert_eq!(
        reasoning::normalize_openai_reasoning_level(ReasoningLevel::Minimal, Some(&supported)),
        Some(ReasoningLevel::Low)
    );
    assert_eq!(
        reasoning::normalize_openai_reasoning_level(
            ReasoningLevel::High,
            Some(&[ReasoningLevel::Off])
        ),
        None
    );
}

#[test]
fn chat_completions_body_uses_each_request_reasoning_level() {
    let provider = OpenAiProvider::new_with_auth(
        "rho-request-reasoning-test".into(),
        Auth::ApiKey("test-key".into()),
        std::sync::Arc::new(crate::credentials::MemoryCredentialStore::default()),
    );
    let messages = [Message::user_text("hello")];
    let low = provider
        .chat_completions_request(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: ReasoningLevel::Low,
                prompt_cache_key: None,
            },
            /*stream*/ false,
        )
        .unwrap();
    let high = provider
        .chat_completions_request(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: ReasoningLevel::High,
                prompt_cache_key: None,
            },
            /*stream*/ true,
        )
        .unwrap();

    assert_eq!(low.reasoning_effort.as_deref(), Some("low"));
    assert_eq!(high.reasoning_effort.as_deref(), Some("high"));
    assert!(!low.stream);
    assert!(high.stream);
}

#[test]
fn codex_responses_body_uses_each_request_reasoning_level() {
    let messages = [Message::user_text("hello")];
    let low = build_codex_responses_body(
        "rho-request-reasoning-test",
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Low,
            prompt_cache_key: None,
        },
    )
    .unwrap();
    let high = build_codex_responses_body(
        "rho-request-reasoning-test",
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert_eq!(
        low["reasoning"],
        json!({"effort": "low", "summary": "auto"})
    );
    assert_eq!(
        high["reasoning"],
        json!({"effort": "high", "summary": "auto"})
    );
}

#[test]
fn codex_responses_body_includes_prompt_cache_key_when_present() {
    let body = build_codex_responses_body(
        "gpt-5-codex",
        ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: Some("rho:session-1"),
        },
    )
    .unwrap();

    assert_eq!(body["prompt_cache_key"], "rho:session-1");
    assert!(body.get("previous_response_id").is_none());
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
}

#[test]
fn codex_responses_body_omits_prompt_cache_key_when_absent() {
    let body = build_codex_responses_body(
        "gpt-5-codex",
        ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert!(body.get("prompt_cache_key").is_none());
}

#[test]
fn codex_responses_body_uses_hosted_web_search_tool() {
    let body = build_codex_responses_body(
        "gpt-5-codex",
        ModelRequest {
            messages: &[Message::user_text("find current docs")],
            tools: &[ToolSpec {
                name: "web_search".into(),
                description: "search the web".into(),
                input_schema: json!({"type": "object"}),
            }],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert_eq!(
        body["tools"],
        json!([{"type": "web_search", "external_web_access": true}])
    );
    assert_eq!(body["tool_choice"], "auto");
}

#[test]
fn chat_completions_request_does_not_serialize_prompt_cache_key() {
    let body = serde_json::to_value(ChatRequest {
        model: "gpt-4.1".into(),
        messages: vec![crate::protocol::openai_chat::OpenAiMessage {
            role: "user".into(),
            content: Some("hello".into()),
            tool_calls: None,
            tool_call_id: None,
        }],
        tools: None,
        tool_choice: None,
        stream: false,
        stream_options: None,
        reasoning: None,
        reasoning_effort: Some("high".into()),
        thinking: None,
    })
    .unwrap();

    assert!(body.get("prompt_cache_key").is_none());
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn extracts_sse_delta_text() {
    let body = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\n",
        "data: [DONE]\n"
    );
    assert_eq!(extract_sse_text(body).unwrap(), "Hello world");
}

#[test]
fn streams_partial_codex_tool_call_arguments() {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    let mut on_event = |event| {
        events.push(event);
        Ok(())
    };

    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":""}}"#,
        &mut state,
        &mut Some(&mut on_event),
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":"}"#,
        &mut state,
        &mut Some(&mut on_event),
    )
    .unwrap();

    assert!(matches!(
        events.as_slice(),
        [
            ModelEvent::ToolCallDelta {
                index: 0,
                id: Some(id),
                name: Some(name),
                arguments,
            },
            ModelEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                arguments: delta,
            }
        ] if id == "call_1" && name == "read_file" && arguments.is_empty() && delta == "{\"path\":"
    ));
}

#[test]
fn chat_stream_usage_normalizes_prompt_cached_tokens() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = None;
    handle_openai_stream_line(
        r#"data: {"usage":{"prompt_tokens":1000,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":700}},"choices":[{"delta":{}}]}"#,
        &mut text,
        &mut tool_calls,
        &mut |event| {
            match event {
                ModelEvent::Usage(event_usage) => usage = Some(event_usage),
                ModelEvent::OutputDelta(_)
                | ModelEvent::ReasoningDelta(_)
                | ModelEvent::ReasoningSummaryDelta(_)
                | ModelEvent::ProviderContext { .. }
                | ModelEvent::WebSearch(_)
                | ModelEvent::RequestAttemptFailed { .. }
                | ModelEvent::ToolCallDelta { .. } => {}
            }
            Ok(())
        },
    )
    .unwrap();

    let usage = usage.unwrap();
    assert_eq!(usage.input_tokens, Some(300));
    assert_eq!(usage.cache_read_tokens, Some(700));
    assert_eq!(usage.output_tokens, Some(20));
    assert_eq!(usage.total_input_tokens(), Some(1000));
}

#[test]
fn codex_response_usage_normalizes_input_cached_tokens() {
    let mut state = CodexSseState::default();
    let mut usage = None;
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"usage":{"input_tokens":1000,"output_tokens":25,"input_tokens_details":{"cached_tokens":700}},"output_text":"done","output":[]}}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::Usage(event_usage) => usage = Some(event_usage),
                ModelEvent::OutputDelta(_)
                | ModelEvent::ReasoningDelta(_)
                | ModelEvent::ReasoningSummaryDelta(_)
                | ModelEvent::ProviderContext { .. }
                | ModelEvent::WebSearch(_)
                | ModelEvent::RequestAttemptFailed { .. }
                | ModelEvent::ToolCallDelta { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    let usage = usage.unwrap();
    assert_eq!(usage.input_tokens, Some(300));
    assert_eq!(usage.cache_read_tokens, Some(700));
    assert_eq!(usage.output_tokens, Some(25));
    assert_eq!(usage.total_input_tokens(), Some(1000));
}

#[test]
fn codex_sse_line_emits_output_delta() {
    let mut state = CodexSseState::default();
    let mut deltas = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_text.delta","delta":"hi"}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::OutputDelta(delta) => deltas.push(delta),
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::WebSearch(_) => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) | ModelEvent::RequestAttemptFailed { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(state.text, "hi");
    assert_eq!(deltas, vec!["hi"]);
    assert!(state.completed_text.is_none());
}

#[test]
fn codex_sse_line_emits_reasoning_summary_delta() {
    let mut state = CodexSseState::default();
    let mut deltas = Vec::new();
    handle_codex_sse_line(
        r#"data:{"type":"response.reasoning_summary_text.delta","delta":"thinking","summary_index":0}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(delta) => deltas.push(delta),
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::WebSearch(_) => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) | ModelEvent::RequestAttemptFailed { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert!(state.text.is_empty());
    assert_eq!(deltas, vec!["thinking"]);
}

#[test]
fn codex_sse_line_emits_reasoning_text_delta() {
    let mut state = CodexSseState::default();
    let mut deltas = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.reasoning_text.delta","delta":"raw","content_index":0}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(delta) => deltas.push(delta),
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::WebSearch(_) => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) | ModelEvent::RequestAttemptFailed { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert!(state.text.is_empty());
    assert_eq!(deltas, vec!["raw"]);
}

#[test]
fn extracts_completed_response_text_when_no_deltas() {
    let body = r#"data: {"type":"response.completed","response":{"output_text":"done","output":null}}
"#;
    assert_eq!(extract_sse_text(body).unwrap(), "done");
}

#[test]
fn completed_response_text_preserves_url_annotations() {
    let body = r#"data: {"type":"response.completed","response":{"output_text":"Rust shipped today.","output":[{"content":[{"text":"Rust shipped today.","annotations":[{"type":"url_citation","title":"Rust Blog","url":"https://blog.rust-lang.org/release"}]}]}]}}
"#;
    let text = extract_sse_text(body).unwrap();

    assert!(text.contains("Rust shipped today."));
    assert!(text.contains("Sources:"));
    assert!(text.contains("Rust Blog: https://blog.rust-lang.org/release"));
}

#[test]
fn codex_sse_line_collects_response_id() {
    let mut state = CodexSseState::default();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"id":"resp_123","output_text":"done","output":null}}"#,
        &mut state,
        &mut None,
    )
    .unwrap();

    let response = state.into_response().unwrap();
    assert_eq!(response.response_id.as_deref(), Some("resp_123"));
}

#[test]
fn codex_sse_line_collects_function_call() {
    let mut state = CodexSseState::default();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call-1","name":"bash","arguments":"{\"command\":\"pwd\"}"}}"#,
        &mut state,
        &mut None,
    )
    .unwrap();

    let response = state.into_response().unwrap();
    let ModelResponse::Assistant(blocks) = response.response;
    assert!(matches!(
        blocks.as_slice(),
        [ContentBlock::ToolCall(ToolCall { id, name, arguments })]
            if id == "call-1" && name == "bash" && arguments == &json!({ "command": "pwd" })
    ));
}

#[test]
fn codex_sse_line_emits_web_search_detail() {
    let mut state = CodexSseState::default();
    let mut searches = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","action":{"type":"search","query":"latest Rust release"}}}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::WebSearch(detail) => searches.push(detail),
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) | ModelEvent::RequestAttemptFailed { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(searches, vec!["for \"latest Rust release\""]);
}

#[test]
fn parses_chat_completion_stream_line_as_output_delta() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut deltas = Vec::new();
    handle_openai_stream_line(
        r#"data: {"choices":[{"delta":{"content":"hé"}}]}"#,
        &mut text,
        &mut tool_calls,
        &mut |event| {
            match event {
                ModelEvent::OutputDelta(delta) => deltas.push(delta),
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::WebSearch(_) => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) | ModelEvent::RequestAttemptFailed { .. } => {}
            }
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(text, "hé");
    assert_eq!(deltas, vec!["hé"]);
    assert!(tool_calls.is_empty());
}

#[test]
fn accumulates_streamed_tool_call_deltas() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    handle_openai_stream_line(
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"bash","arguments":"{\"command\":"}}]}}]}"#,
        &mut text,
        &mut tool_calls,
        &mut |_| Ok(()),
    )
    .unwrap();
    handle_openai_stream_line(
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"pwd\"}"}}]}}]}"#,
        &mut text,
        &mut tool_calls,
        &mut |_| Ok(()),
    )
    .unwrap();

    let response = convert_streamed_response(text, tool_calls).unwrap();
    let ModelResponse::Assistant(blocks) = response;
    assert!(matches!(
        blocks.as_slice(),
        [ContentBlock::ToolCall(ToolCall { id, name, arguments })]
            if id == "call-1" && name == "bash" && arguments == &json!({ "command": "pwd" })
    ));
}

#[test]
fn parses_chat_completion_stream_line_as_reasoning_delta() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut deltas = Vec::new();
    handle_openai_stream_line(
        r#"data: {"choices":[{"delta":{"reasoning_content":"thinking"}}]}"#,
        &mut text,
        &mut tool_calls,
        &mut |event| {
            match event {
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(delta) => deltas.push(delta),
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::WebSearch(_) => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) | ModelEvent::RequestAttemptFailed { .. } => {}
            }
            Ok(())
        },
    )
    .unwrap();

    assert!(text.is_empty());
    assert_eq!(deltas, vec!["thinking"]);
    assert!(tool_calls.is_empty());
}

#[test]
fn serializes_openai_chat_image_content() {
    let message = to_openai_message_for_target(
        Message::User(vec![
            ContentBlock::Text("what is this?".into()),
            ContentBlock::Image(ImageContent {
                data: "aW1n".into(),
                mime_type: "image/png".into(),
            }),
        ]),
        None,
    )
    .unwrap();

    assert_eq!(message.role, "user");
    assert_eq!(
        message.content,
        Some(json!([
            {"type":"text","text":"what is this?"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,aW1n"}}
        ]))
    );
}

#[test]
fn serializes_codex_image_content() {
    let input = codex_input_items(
        vec![Message::User(vec![ContentBlock::Image(ImageContent {
            data: "aW1n".into(),
            mime_type: "image/png".into(),
        })])],
        &mut Vec::new(),
    )
    .unwrap();

    assert_eq!(
        input,
        vec![json!({
            "role":"user",
            "content":[{"type":"input_image","image_url":"data:image/png;base64,aW1n"}]
        })]
    );
}

#[test]
fn serializes_aborted_codex_tool_calls_as_non_executable_context() {
    let input = codex_input_items(
        vec![Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: vec![ContentBlock::Text("partial answer".into())],
            tool_calls: vec![PartialToolCall {
                id: Some("call_1".into()),
                name: Some("read_file".into()),
                arguments: "{\"path\":\"src/".into(),
            }],
            ..AbortedAssistant::default()
        }))],
        &mut Vec::new(),
    )
    .unwrap();

    assert_eq!(
        input,
        vec![json!({
            "role":"assistant",
            "content":"partial answer\n[Partial tool call (not executed)]\nID: call_1\nName: read_file\nArguments:\n{\"path\":\"src/\n[Operation aborted]"
        })]
    );
}

#[test]
fn serializes_openai_native_tool_result() {
    let message = to_openai_message_for_target(
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "done".into(),
        }),
        None,
    )
    .unwrap();
    assert_eq!(message.role, "tool");
    assert_eq!(message.tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(message.content, Some(serde_json::json!("done")));
}
