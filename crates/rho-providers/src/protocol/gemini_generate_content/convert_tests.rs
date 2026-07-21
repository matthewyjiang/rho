use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    convert::{THOUGHT_PART_CONTEXT, THOUGHT_SIGNATURE_CONTEXT},
    *,
};
use crate::model::{
    AssistantMessage, ContentBlock, Message, ModelError, ModelIdentity, ProviderContextBlock,
    ToolCall, ToolResult, ToolSpec,
};

#[test]
fn request_maps_system_tools_results_and_thought_signatures() {
    let identity = ModelIdentity::new("google", "gemini-generate-content", "gemini-2.5-pro");
    let messages = vec![
        Message::System("be concise".into()),
        Message::user_text("run it"),
        Message::assistant(AssistantMessage {
            content: vec![ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: json!({"command":"pwd"}),
            })],
            provenance: Some(identity.clone()),
            provider_context: vec![
                ProviderContextBlock {
                    identity: identity.clone(),
                    kind: THOUGHT_PART_CONTEXT.into(),
                    position: Some(0),
                    data: serde_json::to_value(Part {
                        text: Some("summary".into()),
                        inline_data: None,
                        function_call: None,
                        function_response: None,
                        thought: true,
                        thought_signature: Some("thought-signature".into()),
                    })
                    .unwrap(),
                },
                ProviderContextBlock {
                    identity: identity.clone(),
                    kind: THOUGHT_PART_CONTEXT.into(),
                    position: Some(0),
                    data: serde_json::to_value(Part {
                        text: None,
                        inline_data: None,
                        function_call: None,
                        function_response: None,
                        thought: false,
                        thought_signature: Some("signature-only".into()),
                    })
                    .unwrap(),
                },
                ProviderContextBlock {
                    identity: identity.clone(),
                    kind: THOUGHT_SIGNATURE_CONTEXT.into(),
                    position: Some(0),
                    data: json!("opaque-signature"),
                },
            ],
            ..AssistantMessage::default()
        }),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "/tmp".into(),
        }),
    ];
    let body = build_request(
        &messages,
        &[ToolSpec {
            name: "bash".into(),
            description: "run a command".into(),
            input_schema: json!({"type":"object"}),
        }],
        &identity,
        Some(ThinkingConfig {
            thinking_budget: Some(16_384),
            thinking_level: None,
            include_thoughts: true,
        }),
    )
    .unwrap();
    let value = serde_json::to_value(body).unwrap();

    assert_eq!(value["systemInstruction"]["parts"][0]["text"], "be concise");
    assert_eq!(
        value["contents"][1]["parts"][0],
        json!({
            "text": "summary",
            "thought": true,
            "thoughtSignature": "thought-signature"
        })
    );
    assert_eq!(
        value["contents"][1]["parts"][1],
        json!({"thoughtSignature": "signature-only"})
    );
    assert_eq!(
        value["contents"][1]["parts"][2]["thoughtSignature"],
        "opaque-signature"
    );
    assert_eq!(
        value["contents"][2]["parts"][0]["functionResponse"]["name"],
        "bash"
    );
    assert_eq!(value["tools"][0]["functionDeclarations"][0]["name"], "bash");
    assert_eq!(
        value["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"],
        json!({"type":"object"})
    );
    assert_eq!(
        value["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        16_384
    );
}

#[test]
fn parallel_tool_results_share_one_user_content() {
    let identity = ModelIdentity::new("google", "gemini-generate-content", "gemini-3.5-flash");
    let messages = vec![
        Message::user_text("run both"),
        Message::Assistant(vec![
            ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: json!({"command":"pwd"}),
            }),
            ContentBlock::ToolCall(ToolCall {
                id: "call-2".into(),
                name: "read_file".into(),
                arguments: json!({"path":"README.md"}),
            }),
        ]),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "/tmp".into(),
        }),
        Message::ToolResult(ToolResult {
            id: "call-2".into(),
            ok: true,
            content: "readme".into(),
        }),
    ];

    let body = build_request(&messages, &[], &identity, None).unwrap();

    assert_eq!(body.contents.len(), 3);
    assert_eq!(body.contents[2].role, Some(Role::User));
    assert_eq!(body.contents[2].parts.len(), 2);
    assert_eq!(
        body.contents[2].parts[0]
            .function_response
            .as_ref()
            .map(|response| response.name.as_str()),
        Some("bash")
    );
    assert_eq!(
        body.contents[2].parts[1]
            .function_response
            .as_ref()
            .map(|response| response.name.as_str()),
        Some("read_file")
    );
}

#[test]
fn missing_function_call_ids_stay_local_and_replay_as_absent() {
    fn response() -> GenerateContentResponse {
        serde_json::from_value(json!({
            "candidates": [{
                "content": {"role":"model", "parts":[{"functionCall":{"name":"ping"}}]},
                "finishReason":"STOP"
            }]
        }))
        .unwrap()
    }

    let identity = ModelIdentity::new("google", "gemini-generate-content", "gemini-2.5-flash");
    let mut events = Vec::new();
    let mut first = ResponseCollector::default();
    first
        .apply(
            response(),
            Some(&mut |event| {
                events.push(event);
                Ok(())
            }),
        )
        .unwrap();
    let crate::model::ModelResponse::Assistant(first_content) = first.finish().unwrap();
    let mut second = ResponseCollector::default();
    second.apply(response(), None).unwrap();
    let crate::model::ModelResponse::Assistant(second_content) = second.finish().unwrap();
    let ContentBlock::ToolCall(first_call) = &first_content[0] else {
        panic!("expected first tool call");
    };
    let ContentBlock::ToolCall(second_call) = &second_content[0] else {
        panic!("expected second tool call");
    };
    assert_ne!(first_call.id, second_call.id);
    assert_eq!(first_call.arguments, json!({}));
    let first_call_id = first_call.id.clone();

    let provider_context = events
        .into_iter()
        .filter_map(|event| match event {
            crate::model::ModelEvent::ProviderContext {
                kind,
                position,
                data,
            } => Some(ProviderContextBlock {
                identity: identity.clone(),
                kind,
                position,
                data,
            }),
            _ => None,
        })
        .collect();
    let messages = vec![
        Message::user_text("ping"),
        Message::assistant(AssistantMessage {
            content: first_content,
            provenance: Some(identity.clone()),
            provider_context,
            ..AssistantMessage::default()
        }),
        Message::ToolResult(ToolResult {
            id: first_call_id,
            ok: true,
            content: "pong".into(),
        }),
    ];
    let body = build_request(&messages, &[], &identity, None).unwrap();

    assert_eq!(
        body.contents[1].parts[0].function_call.as_ref().unwrap().id,
        None
    );
    assert_eq!(
        body.contents[2].parts[0]
            .function_response
            .as_ref()
            .unwrap()
            .id,
        None
    );
}

#[test]
fn foreign_reasoning_summary_uses_shared_handoff_format() {
    let target = ModelIdentity::new("google", "gemini-generate-content", "gemini-3.5-flash");
    let messages = vec![
        Message::user_text("question"),
        Message::assistant(AssistantMessage {
            content: vec![ContentBlock::Text("answer".into())],
            provenance: Some(ModelIdentity::new(
                "anthropic",
                "anthropic-messages",
                "claude-test",
            )),
            reasoning_summary: Some("portable summary".into()),
            ..AssistantMessage::default()
        }),
        Message::user_text("continue"),
    ];

    let body = build_request(&messages, &[], &target, None).unwrap();

    assert_eq!(
        body.contents[1].parts[1].text.as_deref(),
        Some("<reasoning_summary>\nportable summary\n</reasoning_summary>")
    );
}

#[test]
fn aborted_assistant_replays_thought_signatures_before_abort_marker() {
    let identity = ModelIdentity::new("google", "gemini-generate-content", "gemini-3.1-flash-lite");
    let messages = vec![
        Message::user_text("run it"),
        Message::AbortedAssistant(Box::new(crate::model::AbortedAssistant {
            // Real cancellation stores text-only content and keeps tool calls separately.
            content: Vec::new(),
            provenance: Some(identity.clone()),
            provider_context: vec![ProviderContextBlock {
                identity: identity.clone(),
                kind: THOUGHT_SIGNATURE_CONTEXT.into(),
                position: Some(0),
                data: json!("opaque-signature"),
            }],
            tool_calls: vec![crate::model::PartialToolCall {
                id: Some("call-1".into()),
                name: Some("bash".into()),
                arguments: r#"{"command":"pwd"}"#.into(),
            }],
            ..crate::model::AbortedAssistant::default()
        })),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "/tmp".into(),
        }),
    ];

    let body = build_request(&messages, &[], &identity, None).unwrap();

    assert_eq!(
        body.contents[1].parts[0]
            .function_call
            .as_ref()
            .and_then(|call| call.id.as_deref()),
        Some("call-1")
    );
    assert_eq!(
        body.contents[1].parts[0].thought_signature.as_deref(),
        Some("opaque-signature")
    );
    assert_eq!(
        body.contents[1].parts[1].text.as_deref(),
        Some("[Operation aborted]")
    );
}

#[test]
fn aborted_parallel_tool_calls_keep_stream_order_not_id_order() {
    let identity = ModelIdentity::new("google", "gemini-generate-content", "gemini-3.1-flash-lite");
    let messages = vec![
        Message::user_text("run both"),
        Message::AbortedAssistant(Box::new(crate::model::AbortedAssistant {
            content: Vec::new(),
            provenance: Some(identity.clone()),
            provider_context: vec![
                ProviderContextBlock {
                    identity: identity.clone(),
                    kind: THOUGHT_SIGNATURE_CONTEXT.into(),
                    position: Some(0),
                    data: json!("sig-z"),
                },
                ProviderContextBlock {
                    identity: identity.clone(),
                    kind: THOUGHT_SIGNATURE_CONTEXT.into(),
                    position: Some(1),
                    data: json!("sig-a"),
                },
            ],
            // Stream order is z then a; lexical ID order is the opposite.
            tool_calls: vec![
                crate::model::PartialToolCall {
                    id: Some("z-call".into()),
                    name: Some("bash".into()),
                    arguments: r#"{"command":"echo z"}"#.into(),
                },
                crate::model::PartialToolCall {
                    id: Some("a-call".into()),
                    name: Some("bash".into()),
                    arguments: r#"{"command":"echo a"}"#.into(),
                },
            ],
            ..crate::model::AbortedAssistant::default()
        })),
    ];

    let body = build_request(&messages, &[], &identity, None).unwrap();
    let parts = &body.contents[1].parts;
    assert_eq!(
        parts[0].function_call.as_ref().unwrap().id.as_deref(),
        Some("z-call")
    );
    assert_eq!(parts[0].thought_signature.as_deref(), Some("sig-z"));
    assert_eq!(
        parts[1].function_call.as_ref().unwrap().id.as_deref(),
        Some("a-call")
    );
    assert_eq!(parts[1].thought_signature.as_deref(), Some("sig-a"));
}

#[test]
fn foreign_aborted_tool_calls_are_not_promoted_to_gemini_function_calls() {
    let target = ModelIdentity::new("google", "gemini-generate-content", "gemini-3.1-flash-lite");
    let messages = vec![
        Message::user_text("continue"),
        Message::AbortedAssistant(Box::new(crate::model::AbortedAssistant {
            content: vec![ContentBlock::Text("partial".into())],
            provenance: Some(ModelIdentity::new("openai", "openai-responses", "gpt-test")),
            tool_calls: vec![crate::model::PartialToolCall {
                id: Some("call-foreign".into()),
                name: Some("bash".into()),
                arguments: r#"{"command":"pwd"}"#.into(),
            }],
            ..crate::model::AbortedAssistant::default()
        })),
        Message::user_text("next"),
    ];

    let body = build_request(&messages, &[], &target, None).unwrap();
    assert_eq!(body.contents[1].parts.len(), 2);
    assert_eq!(body.contents[1].parts[0].text.as_deref(), Some("partial"));
    assert_eq!(
        body.contents[1].parts[1].text.as_deref(),
        Some("[Operation aborted]")
    );
    assert!(body.contents[1]
        .parts
        .iter()
        .all(|part| part.function_call.is_none()));
}

#[test]
fn aborted_mixed_tool_and_text_keeps_signature_targets() {
    let identity = ModelIdentity::new("google", "gemini-generate-content", "gemini-3.1-flash-lite");
    let messages = vec![
        Message::user_text("run"),
        Message::AbortedAssistant(Box::new(crate::model::AbortedAssistant {
            // Cancellation capture now stores tool calls in content at stream positions.
            content: vec![
                ContentBlock::ToolCall(ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    arguments: json!({"command":"pwd"}),
                }),
                ContentBlock::Text("done".into()),
            ],
            provenance: Some(identity.clone()),
            provider_context: vec![
                ProviderContextBlock {
                    identity: identity.clone(),
                    kind: THOUGHT_SIGNATURE_CONTEXT.into(),
                    position: Some(0),
                    data: json!("sig-call"),
                },
                ProviderContextBlock {
                    identity: identity.clone(),
                    kind: THOUGHT_SIGNATURE_CONTEXT.into(),
                    position: Some(1),
                    data: json!("sig-text"),
                },
            ],
            ..crate::model::AbortedAssistant::default()
        })),
    ];

    let body = build_request(&messages, &[], &identity, None).unwrap();
    let parts = &body.contents[1].parts;
    assert_eq!(
        parts[0].function_call.as_ref().unwrap().id.as_deref(),
        Some("call-1")
    );
    assert_eq!(parts[0].thought_signature.as_deref(), Some("sig-call"));
    assert_eq!(parts[1].text.as_deref(), Some("done"));
    assert_eq!(parts[1].thought_signature.as_deref(), Some("sig-text"));
}

#[test]
fn aborted_signed_text_keeps_nonzero_signature_position() {
    let identity = ModelIdentity::new("google", "gemini-generate-content", "gemini-3.1-flash-lite");
    let messages = vec![
        Message::user_text("hi"),
        Message::AbortedAssistant(Box::new(crate::model::AbortedAssistant {
            content: vec![
                ContentBlock::Text("hello".into()),
                ContentBlock::Text(" world".into()),
            ],
            provenance: Some(identity.clone()),
            provider_context: vec![ProviderContextBlock {
                identity: identity.clone(),
                kind: THOUGHT_SIGNATURE_CONTEXT.into(),
                position: Some(1),
                data: json!("sig-world"),
            }],
            ..crate::model::AbortedAssistant::default()
        })),
        Message::user_text("next"),
    ];

    let body = build_request(&messages, &[], &identity, None).unwrap();
    assert_eq!(body.contents[1].parts[0].text.as_deref(), Some("hello"));
    assert!(body.contents[1].parts[0].thought_signature.is_none());
    assert_eq!(body.contents[1].parts[1].text.as_deref(), Some(" world"));
    assert_eq!(
        body.contents[1].parts[1].thought_signature.as_deref(),
        Some("sig-world")
    );
}

#[test]
fn signed_text_parts_are_not_merged_with_neighbors() {
    let mut collector = ResponseCollector::default();
    let mut events = Vec::new();
    collector
        .apply(
            serde_json::from_value(json!({
                "candidates":[{"content":{"parts":[
                    {"text":"hello"},
                    {"text":" world","thoughtSignature":"sig-1"},
                    {"text":"!"}
                ]}}]
            }))
            .unwrap(),
            Some(&mut |event| {
                events.push(event);
                Ok(())
            }),
        )
        .unwrap();

    assert_eq!(
        collector.finish().unwrap(),
        crate::model::ModelResponse::Assistant(vec![
            ContentBlock::Text("hello".into()),
            ContentBlock::Text(" world".into()),
            ContentBlock::Text("!".into()),
        ])
    );
    assert!(events.iter().any(|event| matches!(
        event,
        crate::model::ModelEvent::ProviderContext {
            kind,
            position: Some(1),
            data,
        } if kind == THOUGHT_SIGNATURE_CONTEXT && data == "sig-1"
    )));
}

#[test]
fn blocked_response_reports_usage_before_error() {
    let response: GenerateContentResponse = serde_json::from_value(json!({
        "promptFeedback": {"blockReason":"SAFETY"},
        "usageMetadata": {"promptTokenCount":4,"totalTokenCount":4}
    }))
    .unwrap();
    let mut events = Vec::new();
    let error = ResponseCollector::default()
        .apply(
            response,
            Some(&mut |event| {
                events.push(event);
                Ok(())
            }),
        )
        .unwrap_err();

    assert!(matches!(error, ModelError::InvalidResponse(message) if message.contains("blocked")));
    assert!(matches!(
        events.as_slice(),
        [crate::model::ModelEvent::Usage(usage)] if usage.total_tokens == Some(4)
    ));
}

#[test]
fn usage_deltas_preserve_omitted_cumulative_fields() {
    let mut collector = ResponseCollector::default();
    let mut usages = Vec::new();
    for response in [
        json!({
            "candidates":[{"content":{"parts":[{"text":"a"}]}}],
            "usageMetadata":{"promptTokenCount":10,"cachedContentTokenCount":3,"candidatesTokenCount":4,"thoughtsTokenCount":2,"totalTokenCount":16}
        }),
        json!({
            "candidates":[{"content":{"parts":[{"text":"b"}]},"finishReason":"STOP"}],
            "usageMetadata":{"candidatesTokenCount":7,"totalTokenCount":19}
        }),
    ] {
        collector
            .apply(
                serde_json::from_value(response).unwrap(),
                Some(&mut |event| {
                    if let crate::model::ModelEvent::Usage(usage) = event {
                        usages.push(usage);
                    }
                    Ok(())
                }),
            )
            .unwrap();
    }

    assert_eq!(
        collector.finish().unwrap(),
        crate::model::ModelResponse::Assistant(vec![ContentBlock::Text("ab".into())])
    );
    assert_eq!(usages[0].input_tokens, Some(7));
    assert_eq!(usages[0].cache_read_tokens, Some(3));
    assert_eq!(usages[0].output_tokens, Some(6));
    assert_eq!(usages[1].input_tokens, Some(0));
    assert_eq!(usages[1].cache_read_tokens, Some(0));
    assert_eq!(usages[1].output_tokens, Some(3));
    assert_eq!(usages[1].total_tokens, Some(3));
}

#[test]
fn response_maps_reasoning_tools_signatures_and_usage() {
    let response: GenerateContentResponse = serde_json::from_value(json!({
        "candidates": [{"content":{"role":"model","parts":[
            {"text":"working", "thought":true, "thoughtSignature":"thought-sig"},
            {"functionCall":{"id":"call-7","name":"bash","args":{"command":"pwd"}}, "thoughtSignature":"sig"}
        ]}, "finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":10,"cachedContentTokenCount":3,"candidatesTokenCount":4,"thoughtsTokenCount":2,"totalTokenCount":16}
    })).unwrap();
    let mut events = Vec::new();
    let mut callback = |event| {
        events.push(event);
        Ok(())
    };
    let mut collector = ResponseCollector::default();
    collector.apply(response, Some(&mut callback)).unwrap();
    let output = collector.finish().unwrap();

    assert_eq!(
        output,
        crate::model::ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-7".into(),
            name: "bash".into(),
            arguments: json!({"command":"pwd"})
        })])
    );
    assert!(events.iter().any(
        |event| matches!(event, crate::model::ModelEvent::ReasoningSummaryDelta(text) if text == "working")
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        crate::model::ModelEvent::ProviderContext { kind, data, .. }
            if kind == THOUGHT_PART_CONTEXT && data["thoughtSignature"] == "thought-sig"
    )));
    assert!(events.iter().any(|event| matches!(event, crate::model::ModelEvent::ProviderContext { data, .. } if data == "sig")));
    assert!(events.iter().any(|event| matches!(event, crate::model::ModelEvent::Usage(usage) if usage.input_tokens == Some(7) && usage.cache_read_tokens == Some(3) && usage.output_tokens == Some(6))));
}

#[test]
fn transient_finish_reason_is_reported_as_retryable() {
    let response: GenerateContentResponse = serde_json::from_value(json!({
        "candidates": [{"finishReason": "MALFORMED_FUNCTION_CALL"}],
        "usageMetadata": {"promptTokenCount": 4, "totalTokenCount": 4}
    }))
    .unwrap();
    let error = ResponseCollector::default()
        .apply(response, None)
        .unwrap_err();

    assert!(matches!(
        error,
        ModelError::ProviderReported {
            kind: crate::model::ProviderReportedErrorKind::Unavailable,
            ..
        }
    ));
}

#[test]
fn content_policy_finish_reason_stays_a_permanent_invalid_response() {
    let response: GenerateContentResponse = serde_json::from_value(json!({
        "candidates": [{"finishReason": "SAFETY"}],
        "usageMetadata": {"promptTokenCount": 4, "totalTokenCount": 4}
    }))
    .unwrap();
    let error = ResponseCollector::default()
        .apply(response, None)
        .unwrap_err();

    assert!(matches!(error, ModelError::InvalidResponse(_)));
}
