use super::*;
use crate::model::{
    AbortedAssistant, ContentBlock, Message, PartialToolCall, ReasoningCapabilities,
    ReasoningLevelSet, ToolCall, ToolSpec,
};
use crate::protocol::openai_chat::ChatStreamAccumulator;
use crate::protocol::openai_responses::{
    codex_input_items, codex_reasoning_param, extract_sse_text, handle_codex_sse_line,
    CodexSseState,
};
use crate::reasoning::ReasoningLevel;
use pretty_assertions::assert_eq;
use serde_json::json;

// Covers: Codex must warn only when the server declines a requested priority tier.
// Owner: OpenAI provider request policy
#[test]
fn reported_service_tier_detects_priority_fallback() {
    let cases = [
        (Some("priority"), false),
        (Some("default"), true),
        (None, false),
    ];

    for (reported, should_report) in cases {
        let mut events = Vec::new();
        let mut on_event = |event: ModelEvent| {
            events.push(event);
            Ok(())
        };
        let mut on_event =
            Some(&mut on_event as &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send));

        report_service_tier_fallback(
            Some(rho_sdk::model::ServiceTier::Priority),
            reported,
            &mut on_event,
        )
        .unwrap();

        assert_eq!(
            events.len(),
            usize::from(should_report),
            "reported={reported:?}"
        );
        if should_report {
            assert_eq!(
                events,
                vec![ModelEvent::ServiceTierFallback {
                    requested: rho_sdk::model::ServiceTier::Priority,
                    used: "default".into(),
                }]
            );
        }
    }
}

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

// Covers: `/responses/compact` is only advertised for first-party OpenAI
// hosts; catalog gateways reusing this transport (opencode-go) fall straight
// to portable summary compaction instead of a guaranteed-failing round trip.
// Owner: OpenAI provider native compaction policy
#[test]
fn native_compact_is_advertised_only_for_first_party_hosts() {
    use rho_sdk::provider::ModelProvider as _;

    let cases: [(&'static str, bool); 3] = [
        ("openai", true),
        ("openai-codex", true),
        ("opencode-go", false),
    ];
    let messages = [Message::user_text("hello")];

    for (identity_provider, expected) in cases {
        let provider = OpenAiProvider::new_with_identity(
            "test-model".into(),
            Some(Auth::ApiKey("test-key".into())),
            provider_client(),
            None,
            /*hosted_web_search*/ false,
            identity_provider,
        );
        let request = ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Off,
            prompt_cache_key: None,
        };

        assert_eq!(
            provider.native_compact(request).is_some(),
            expected,
            "identity_provider={identity_provider}"
        );
    }
}

#[tokio::test]
async fn api_responses_body_uses_each_request_reasoning_level() {
    let provider = OpenAiProvider::new_with_auth(
        "rho-request-reasoning-test".into(),
        Auth::ApiKey("test-key".into()),
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
        .await
        .unwrap();
    let high = provider
        .openai_api_responses_body(ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: None,
        })
        .await
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

#[tokio::test]
async fn codex_responses_body_includes_prompt_cache_key_when_present() {
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

#[tokio::test]
async fn codex_responses_body_uses_hosted_web_search_tool() {
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
            | ModelEvent::ProviderContext { .. }
            | ModelEvent::Usage(_)
            | ModelEvent::GenerationOutputTokens(_)
            | ModelEvent::HostedToolActivity { .. }
            | ModelEvent::ServiceTierFallback { .. } => None,
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
            | ModelEvent::ProviderContext { .. }
            | ModelEvent::Usage(_)
            | ModelEvent::GenerationOutputTokens(_)
            | ModelEvent::HostedToolActivity { .. }
            | ModelEvent::ServiceTierFallback { .. } => None,
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

// Covers: Chat Completions must publish generation-only throughput before the
// normalized usage snapshot once at stream finish
// Owner: OpenAI Chat Completions protocol
#[test]
fn chat_stream_usage_normalizes_prompt_cache_tokens() {
    let mut chat_stream = ChatStreamAccumulator::default();
    let mut events = Vec::new();
    let mut on_event = |event: ModelEvent| {
        events.push(event);
        Ok(())
    };
    chat_stream
        .handle_line(
            r#"data: {"choices":[{"delta":{"content":"ok"}}]}"#,
            &mut on_event,
        )
        .unwrap();
    chat_stream
        .handle_line(
            r#"data: {"usage":{"prompt_tokens":1000,"completion_tokens":20,"total_tokens":1020,"prompt_tokens_details":{"cached_tokens":700,"cache_write_tokens":200},"completion_tokens_details":{"reasoning_tokens":5}},"choices":[{"delta":{}}]}"#,
            &mut on_event,
        )
        .unwrap();
    chat_stream.finish(&mut on_event).unwrap();

    assert_eq!(
        events,
        vec![
            ModelEvent::OutputDelta("ok".into()),
            ModelEvent::GenerationOutputTokens(rho_sdk::model::GenerationOutputTokens::Reported(
                15
            ),),
            ModelEvent::Usage(crate::model::ModelUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_tokens: Some(700),
                cache_write_tokens: Some(200),
                total_tokens: Some(1020),
                ..crate::model::ModelUsage::default()
            }),
        ]
    );
}

// Covers: Responses must keep generation and aggregate counts on the same
// envelope source while emitting generation-only throughput before usage.
// Owner: OpenAI Responses protocol
#[test]
fn codex_response_usage_normalizes_input_cache_tokens() {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","usage":{"output_tokens":99,"output_tokens_details":{"reasoning_tokens":1}},"response":{"usage":{"input_tokens":1000,"output_tokens":25,"total_tokens":1025,"input_tokens_details":{"cached_tokens":700,"cache_write_tokens":200},"output_tokens_details":{"reasoning_tokens":7}},"output_text":"done","output":[]}}"#,
        &mut state,
        &mut Some(&mut |event| {
            events.push(event);
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(
        events,
        vec![
            ModelEvent::GenerationOutputTokens(rho_sdk::model::GenerationOutputTokens::Reported(
                18
            ),),
            ModelEvent::Usage(crate::model::ModelUsage {
                input_tokens: Some(100),
                output_tokens: Some(25),
                cache_read_tokens: Some(700),
                cache_write_tokens: Some(200),
                total_tokens: Some(1025),
                ..crate::model::ModelUsage::default()
            }),
        ]
    );
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
                ModelEvent::Usage(_) => {}
                ModelEvent::GenerationOutputTokens(_)
                | ModelEvent::HostedToolActivity { .. }
                | ModelEvent::ServiceTierFallback { .. } => {}
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
                                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) => {}
                ModelEvent::GenerationOutputTokens(_)
                | ModelEvent::HostedToolActivity { .. }
                | ModelEvent::ServiceTierFallback { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(searches, vec!["latest Rust release".to_string()]);
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
                ModelEvent::HostedToolActivity { name, detail } => {
                    searches.push((name, detail));
                }
                ModelEvent::WebSearch(_) => {}
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) => {}
                ModelEvent::GenerationOutputTokens(_) | ModelEvent::ServiceTierFallback { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(
        searches,
        vec![(
            "x_search".to_string(),
            "what people say about xAI".to_string()
        )]
    );
}

#[test]
fn codex_sse_search_activity_is_not_duplicated_on_completed() {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    let mut collect = |event: ModelEvent| {
        match event {
            ModelEvent::WebSearch(detail) => events.push(("web_search".into(), detail)),
            ModelEvent::HostedToolActivity { name, detail } => {
                events.push((name, detail));
            }
            _ => {}
        }
        Ok(())
    };

    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","action":{"type":"search","query":"latest Rust release"}}}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"x_search_call","id":"xs_1","action":{"type":"search","query":"what people say about xAI"}}}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"web_search_call","id":"ws_1","action":{"type":"search","query":"latest Rust release"}},{"type":"x_search_call","id":"xs_1","action":{"type":"search","query":"what people say about xAI"}},{"type":"message","content":[{"text":"done"}]}]}}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();

    assert_eq!(
        events,
        vec![
            ("web_search".to_string(), "latest Rust release".to_string()),
            (
                "x_search".to_string(),
                "what people say about xAI".to_string()
            ),
        ]
    );
}

// Covers: xAI emits hosted X searches as custom_tool_call items with JSON input.
// Owner: providers stream parse
#[test]
fn codex_sse_line_emits_x_search_from_custom_tool_call() {
    let mut state = CodexSseState::default();
    let mut searches = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"call_id":"xs_call-557c03fd-0","input":"{\"query\":\"codex reset\",\"limit\":\"10\",\"mode\":\"Latest\"}","name":"x_keyword_search","type":"custom_tool_call","id":"ctc_response_call-0","status":"completed"},"output_index":1}"#,
        &mut state,
        &mut Some(&mut |event| {
            match event {
                ModelEvent::HostedToolActivity { name, detail } => {
                    searches.push((name, detail));
                }
                ModelEvent::WebSearch(_) => {}
                ModelEvent::OutputDelta(_) => {}
                ModelEvent::ReasoningDelta(_) => {}
                ModelEvent::ReasoningSummaryDelta(_) => {}
                ModelEvent::ProviderContext { .. } => {}
                ModelEvent::ToolCallDelta { .. } => {}
                ModelEvent::Usage(_) => {}
                ModelEvent::GenerationOutputTokens(_) | ModelEvent::ServiceTierFallback { .. } => {}
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(
        searches,
        vec![("x_search".to_string(), "codex reset".to_string())]
    );
}

// Covers: ordinary custom_tool_call items must not be labeled hosted x_search.
// Owner: providers stream parse
#[test]
fn codex_sse_custom_tool_call_without_xs_call_id_is_not_x_search() {
    let mut state = CodexSseState::default();
    let mut searches = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"call_id":"call_other-1","input":"{\"query\":\"nope\"}","name":"x_keyword_search","type":"custom_tool_call","id":"ctc_1","status":"completed"}}"#,
        &mut state,
        &mut Some(&mut |event| {
            if let ModelEvent::HostedToolActivity { name, detail } = event {
                searches.push((name, detail));
            }
            Ok(())
        }),
    )
    .unwrap();

    assert!(
        searches.is_empty(),
        "unexpected hosted activity: {searches:?}"
    );
}

// Covers: search activity on completed must surface even when the stream already has text
// Owner: providers stream parse
#[test]
fn codex_sse_completed_emits_search_activity_when_stream_has_text() {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    let mut collect = |event: ModelEvent| {
        match event {
            ModelEvent::WebSearch(detail) => events.push(("web_search".into(), detail)),
            ModelEvent::HostedToolActivity { name, detail } => {
                events.push((name, detail));
            }
            _ => {}
        }
        Ok(())
    };

    // Simulate text already streamed before completed (common after hosted search).
    handle_codex_sse_line(
        r#"data: {"type":"response.output_text.delta","delta":"answer"}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"x_search_call","id":"xs_1","name":"x_keyword_search","arguments":"{\"query\":\"next codex reset\"}"},{"type":"message","content":[{"type":"output_text","text":"answer"}]}]}}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();

    assert_eq!(
        events,
        vec![("x_search".to_string(), "next codex reset".to_string())]
    );
}

#[test]
fn codex_sse_completed_processes_unstreamed_items_individually() {
    let mut state = CodexSseState::default();
    let mut contexts = Vec::new();
    let mut collect = |event: ModelEvent| {
        if let ModelEvent::ProviderContext { data, .. } = event {
            contexts.push(data);
        }
        Ok(())
    };

    handle_codex_sse_line(
        r#"data: {"type":"response.output_text.delta","delta":"answer"}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"output":[{"type":"reasoning","id":"rs_1","encrypted_content":"opaque"},{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"},{"type":"message","id":"msg_1","content":[{"type":"output_text","text":"answer"}]}]}}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();

    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["id"], "rs_1");
    assert_eq!(state.tool_calls.len(), 1);
    assert_eq!(state.tool_calls[0].id, "call_1");
}

// Covers: completed envelope with tool call + message must keep text when no deltas streamed.
// Owner: openai_shared stream assembly
#[test]
fn completed_envelope_keeps_message_text_beside_function_call_without_deltas() {
    let mut state = CodexSseState::default();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"id":"resp_mixed","output":[{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"},{"type":"message","id":"msg_1","content":[{"type":"output_text","text":"here is the file"}]}]}}"#,
        &mut state,
        &mut None,
    )
    .unwrap();

    let response = state.into_response().unwrap().response;
    assert!(
        matches!(
            response,
            ModelResponse::Assistant(ref blocks)
                if matches!(
                    blocks.as_slice(),
                    [
                        ContentBlock::Text(text),
                        ContentBlock::ToolCall(ToolCall {
                            id,
                            name,
                            ..
                        })
                    ] if text == "here is the file"
                        && id == "call_1"
                        && name == "read_file"
                )
        ),
        "{response:?}"
    );
}

// Covers: empty SSE completions must surface a stream summary, not a bare label.
// Owner: openai_shared stream diagnostics
#[test]
fn empty_codex_sse_content_error_includes_stream_summary() {
    let mut state = CodexSseState::default();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1"}}"#,
        &mut state,
        &mut None,
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","action":{"type":"search","query":"rho"}}}"#,
        &mut state,
        &mut None,
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"id":"resp_empty","status":"completed","service_tier":"default","output":[{"type":"reasoning","id":"rs_1"},{"type":"web_search_call","id":"ws_1"}]}}"#,
        &mut state,
        &mut None,
    )
    .unwrap();

    let err = state.into_response().unwrap_err();
    let message = match err {
        ModelError::InvalidResponse(message) => message,
        other => panic!("expected InvalidResponse, got {other:?}"),
    };

    assert!(
        message.starts_with("missing response content in SSE ("),
        "{message}"
    );
    for needle in [
        "completed=true",
        "response_id=resp_empty",
        "status=completed",
        "service_tier=default",
        "streamed_text_chars=0",
        "images=0",
        "tool_calls=0",
        "output_items=[reasoning, web_search_call]",
        "events=[response.completed, response.output_item.done]",
    ] {
        assert!(message.contains(needle), "missing {needle:?} in {message}");
    }
}

#[test]
fn codex_sse_search_without_detail_still_emits_activity() {
    let mut state = CodexSseState::default();
    let mut searches = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"x_search_call","id":"xs_1","status":"completed"}}"#,
        &mut state,
        &mut Some(&mut |event| {
            if let ModelEvent::HostedToolActivity { name, detail } = event {
                searches.push((name, detail));
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(searches, vec![("x_search".to_string(), String::new())]);
}

#[test]
fn codex_sse_search_query_count_ignores_invalid_entries() {
    let mut state = CodexSseState::default();
    let mut details = Vec::new();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"x_search_call","id":"xs_1","arguments":"{\"queries\":[\"a\",\"\",7,\"b\",\"c\",\"d\"]}"}}"#,
        &mut state,
        &mut Some(&mut |event| {
            if let ModelEvent::HostedToolActivity { detail, .. } = event {
                details.push(detail);
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(details, vec!["a · b · c · 1 more"]);
}

// Covers: action url/pattern details stay bare payload values (no opened/found chrome).
// Owner: providers stream parse
#[test]
fn codex_sse_search_action_url_and_pattern_are_bare_detail() {
    let mut state = CodexSseState::default();
    let mut details = Vec::new();
    let mut collect = |event: ModelEvent| {
        match event {
            ModelEvent::WebSearch(detail) => details.push(detail),
            ModelEvent::HostedToolActivity { detail, .. } => details.push(detail),
            _ => {}
        }
        Ok(())
    };

    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","action":{"type":"open_page","url":"https://example.com/docs"}}}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();
    handle_codex_sse_line(
        r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_2","action":{"type":"find","pattern":"TODO(foo)"}}}"#,
        &mut state,
        &mut Some(&mut collect),
    )
    .unwrap();

    assert_eq!(
        details,
        vec![
            "https://example.com/docs".to_string(),
            "TODO(foo)".to_string(),
        ]
    );
}

#[test]
fn serializes_aborted_codex_tool_calls_as_non_executable_context() {
    let input = codex_input_items(
        &[Message::AbortedAssistant(Box::new(AbortedAssistant {
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
