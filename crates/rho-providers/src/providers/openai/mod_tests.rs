use super::*;
use crate::model::{
    AbortedAssistant, ContentBlock, Message, PartialToolCall, ReasoningCapabilities,
    ReasoningLevelSet, ToolCall, ToolSpec,
};
use crate::protocol::openai_chat::{convert_streamed_response, handle_openai_stream_line};
use crate::protocol::openai_responses::{
    codex_input_items, codex_reasoning_param, extract_sse_text, handle_codex_sse_line,
    CodexSseState,
};
use crate::reasoning::ReasoningLevel;
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
fn immutable_openai_reasoning_profile_normalizes_exact_and_omits_fixed_models() {
    let exact = reasoning::OpenAiReasoningProfile::from_metadata(Some(
        crate::model::models_dev::ModelMetadata {
            supported_reasoning_levels: Some(vec![ReasoningLevel::Low, ReasoningLevel::High]),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..Default::default()
        },
    ));
    assert_eq!(
        exact
            .config("openai", "gpt-test", ReasoningLevel::Off)
            .unwrap(),
        reasoning::OpenAiReasoningConfig {
            effort: Some("low".into()),
            summary: Some("auto".into()),
        }
    );

    let fixed = reasoning::OpenAiReasoningProfile::from_metadata(Some(
        crate::model::models_dev::ModelMetadata {
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..Default::default()
        },
    ));
    assert_eq!(
        fixed
            .config("openai", "gpt-test", ReasoningLevel::High)
            .unwrap(),
        reasoning::OpenAiReasoningConfig {
            effort: None,
            summary: None,
        }
    );
}

#[test]
fn openai_reasoning_normalization_never_turns_requested_reasoning_off() {
    let capabilities = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::High,
    ]));
    assert_eq!(
        reasoning::normalize_openai_reasoning_level(ReasoningLevel::Minimal, &capabilities),
        Some(ReasoningLevel::Low)
    );
    let off_only = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![ReasoningLevel::Off]));
    assert_eq!(
        reasoning::normalize_openai_reasoning_level(ReasoningLevel::High, &off_only),
        None
    );
    let mandatory = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Low,
        ReasoningLevel::High,
    ]));
    assert_eq!(
        reasoning::normalize_openai_reasoning_level(ReasoningLevel::Off, &mandatory),
        Some(ReasoningLevel::Low)
    );
    assert_eq!(
        reasoning::normalize_openai_reasoning_level(
            ReasoningLevel::High,
            &ReasoningCapabilities::NotConfigurable,
        ),
        Some(ReasoningLevel::High)
    );
    assert_eq!(
        reasoning::normalize_openai_reasoning_level(
            ReasoningLevel::High,
            &ReasoningCapabilities::Unknown,
        ),
        Some(ReasoningLevel::High)
    );
}

#[test]
fn api_responses_body_uses_each_request_reasoning_level() {
    let provider = OpenAiProvider::new_with_auth(
        "rho-request-reasoning-test".into(),
        Auth::ApiKey("test-key".into()),
        std::sync::Arc::new(crate::credentials::MemoryCredentialStore::default()),
    );
    let messages = [Message::user_text("hello")];
    let low = provider
        .openai_api_responses_body(ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Low,
            prompt_cache_key: None,
        })
        .unwrap();
    let high = provider
        .openai_api_responses_body(ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: None,
        })
        .unwrap();

    assert_eq!(
        low["reasoning"],
        json!({"effort": "low", "summary": "auto"})
    );
    assert_eq!(
        high["reasoning"],
        json!({"effort": "high", "summary": "auto"})
    );
    assert_eq!(low["stream"], true);
    assert_eq!(high["stream"], true);
    assert_eq!(low["include"], json!(["reasoning.encrypted_content"]));
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
fn completed_tool_call_item_publishes_arguments_that_never_streamed() {
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
        r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}"#,
        &mut state,
        &mut Some(&mut on_event),
    )
    .unwrap();

    assert!(
        matches!(
            events.as_slice(),
            [
                ModelEvent::ToolCallDelta { arguments, .. },
                ModelEvent::ToolCallDelta {
                    index: 0,
                    id: Some(id),
                    name: Some(name),
                    arguments: completed,
                }
            ] if arguments.is_empty()
                && id == "call_1"
                && name == "read_file"
                && completed == r#"{"path":"src/main.rs"}"#
        ),
        "{events:?}"
    );
}

#[test]
fn completed_tool_call_arguments_are_published_exactly_once() {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    let mut on_event = |event| {
        events.push(event);
        Ok(())
    };

    for line in [
        r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":""}}"#,
        r#"data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":"}"#,
        r#"data: {"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"src/main.rs\"}"}"#,
        r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}"#,
    ] {
        handle_codex_sse_line(line, &mut state, &mut Some(&mut on_event)).unwrap();
    }

    let streamed = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallDelta { arguments, .. } => Some(arguments.as_str()),
            ModelEvent::OutputDelta(_)
            | ModelEvent::ReasoningDelta(_)
            | ModelEvent::ReasoningSummaryDelta(_)
            | ModelEvent::WebSearch(_)
            | ModelEvent::XSearch(_)
            | ModelEvent::ProviderContext { .. }
            | ModelEvent::Usage(_) => None,
        })
        .collect::<String>();
    assert_eq!(streamed, r#"{"path":"src/main.rs"}"#);
}

#[test]
fn parallel_tool_calls_stream_arguments_per_output_index() {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    let mut on_event = |event| {
        events.push(event);
        Ok(())
    };

    for line in [
        r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":""}}"#,
        r#"data: {"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call_2","name":"read_file","arguments":""}}"#,
        r#"data: {"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","call_id":"call_2","name":"read_file","arguments":"{\"path\":\"b.rs\"}"}}"#,
        r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"a.rs\"}"}}"#,
    ] {
        handle_codex_sse_line(line, &mut state, &mut Some(&mut on_event)).unwrap();
    }

    let published = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallDelta {
                index, arguments, ..
            } => (!arguments.is_empty()).then(|| (*index, arguments.clone())),
            ModelEvent::OutputDelta(_)
            | ModelEvent::ReasoningDelta(_)
            | ModelEvent::ReasoningSummaryDelta(_)
            | ModelEvent::WebSearch(_)
            | ModelEvent::XSearch(_)
            | ModelEvent::ProviderContext { .. }
            | ModelEvent::Usage(_) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        published,
        vec![
            (1, r#"{"path":"b.rs"}"#.to_string()),
            (0, r#"{"path":"a.rs"}"#.to_string()),
        ]
    );
}

#[test]
fn chat_stream_usage_normalizes_prompt_cache_tokens() {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = None;
    handle_openai_stream_line(
        r#"data: {"usage":{"prompt_tokens":1000,"completion_tokens":20,"total_tokens":1020,"prompt_tokens_details":{"cached_tokens":700,"cache_write_tokens":200}},"choices":[{"delta":{}}]}"#,
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
                | ModelEvent::XSearch(_)
                | ModelEvent::ToolCallDelta { .. } => {}
            }
            Ok(())
        },
    )
    .unwrap();

    let usage = usage.unwrap();
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.cache_read_tokens, Some(700));
    assert_eq!(usage.cache_write_tokens, Some(200));
    assert_eq!(usage.output_tokens, Some(20));
    assert_eq!(usage.total_input_tokens(), Some(1000));
    assert_eq!(usage.total_tokens, Some(1020));
}

#[test]
fn codex_response_usage_normalizes_input_cache_tokens() {
    let mut state = CodexSseState::default();
    let mut usage = None;
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"usage":{"input_tokens":1000,"output_tokens":25,"total_tokens":1025,"input_tokens_details":{"cached_tokens":700,"cache_write_tokens":200}},"output_text":"done","output":[]}}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::Usage(event_usage) => usage = Some(event_usage),
                ModelEvent::OutputDelta(_)
                | ModelEvent::ReasoningDelta(_)
                | ModelEvent::ReasoningSummaryDelta(_)
                | ModelEvent::ProviderContext { .. }
                | ModelEvent::WebSearch(_)
                | ModelEvent::XSearch(_)
                | ModelEvent::ToolCallDelta { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    let usage = usage.unwrap();
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.cache_read_tokens, Some(700));
    assert_eq!(usage.cache_write_tokens, Some(200));
    assert_eq!(usage.output_tokens, Some(25));
    assert_eq!(usage.total_input_tokens(), Some(1000));
    assert_eq!(usage.total_tokens, Some(1025));
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
                ModelEvent::XSearch(_) => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert!(state.text.is_empty());
    assert_eq!(deltas, vec!["thinking"]);
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
fn codex_sse_line_emits_web_search_detail() {
    let mut state = CodexSseState::default();
    let mut searches = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","action":{"type":"search","query":"latest Rust release"}}}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::WebSearch(detail) => searches.push(detail),
                ModelEvent::XSearch(_) => {}
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(searches, vec!["for \"latest Rust release\"".to_string()]);
}

#[test]
fn codex_sse_line_emits_x_search_detail() {
    let mut state = CodexSseState::default();
    let mut searches = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"x_search_call","id":"xs_1","action":{"type":"search","query":"what people say about xAI"}}}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::XSearch(detail) => searches.push(detail),
                ModelEvent::WebSearch(_) => {}
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(
        searches,
        vec!["for \"what people say about xAI\"".to_string()]
    );
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
