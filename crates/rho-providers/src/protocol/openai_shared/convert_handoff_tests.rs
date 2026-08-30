use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn chat_handoff_keeps_foreign_reasoning_summary_as_tagged_text() {
    let source = crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
    let target =
        crate::model::ModelIdentity::new("openai", "openai-chat-completions", "gpt-chat-test");
    let message = Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(source),
        reasoning_summary: Some("verified it".into()),
        provider_context: Vec::new(),
    });

    let converted = to_openai_message_for_target(&message, Some(&target)).unwrap();
    let content = converted.content.unwrap().as_str().unwrap().to_string();

    assert!(content.contains("answer"));
    assert!(content.contains("<reasoning_summary>"));
    assert!(content.contains("verified it"));
}

// Covers: Qwen-style reasoning_content must replay on same-model tool loops
// Owner: openai chat completions history conversion
#[test]
fn chat_handoff_replays_reasoning_content_for_exact_model() {
    let identity = crate::model::ModelIdentity::new(
        "qwen-token-plan",
        "openai-chat-completions",
        "qwen3.8-max",
    );
    let message = Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: json!({"command": "pwd"}),
        })],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: vec![crate::model::ProviderContextBlock {
            identity: identity.clone(),
            kind: OPENAI_CHAT_REASONING_CONTENT_KIND.into(),
            position: Some(0),
            data: json!("need to inspect the workspace first"),
        }],
    });

    let converted = to_openai_message_for_target(&message, Some(&identity)).unwrap();
    assert_eq!(
        converted.reasoning_content.as_deref(),
        Some("need to inspect the workspace first")
    );
    assert!(converted.content.is_none());
    let tool_calls = converted.tool_calls.expect("tool calls");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_1");
    assert_eq!(tool_calls[0].function.name, "bash");
}

// Covers: foreign targets must not receive raw reasoning_content
// Owner: openai chat completions history conversion
#[test]
fn chat_handoff_omits_reasoning_content_for_foreign_model() {
    let source = crate::model::ModelIdentity::new(
        "qwen-token-plan",
        "openai-chat-completions",
        "qwen3.8-max",
    );
    let target = crate::model::ModelIdentity::new("openrouter", "openai-chat-completions", "other");
    let message = Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(source.clone()),
        reasoning_summary: None,
        provider_context: vec![crate::model::ProviderContextBlock {
            identity: source,
            kind: OPENAI_CHAT_REASONING_CONTENT_KIND.into(),
            position: Some(0),
            data: json!("private thoughts"),
        }],
    });

    let converted = to_openai_message_for_target(&message, Some(&target)).unwrap();
    assert!(converted.reasoning_content.is_none());
    assert_eq!(converted.content.unwrap().as_str().unwrap(), "answer");
}

// Covers: unknown chat replay kinds must fail instead of silent drop
// Owner: openai chat completions history conversion
#[test]
fn chat_handoff_rejects_unknown_replay_context_kinds() {
    let identity = crate::model::ModelIdentity::new(
        "qwen-token-plan",
        "openai-chat-completions",
        "qwen3.8-max",
    );
    let message = Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: vec![crate::model::ProviderContextBlock {
            identity: identity.clone(),
            kind: "future_chat_kind".into(),
            position: Some(0),
            data: json!({"x": 1}),
        }],
    });

    let err = match to_openai_message_for_target(&message, Some(&identity)) {
        Ok(_) => panic!("expected unknown replay kind to fail"),
        Err(error) => error,
    };
    assert!(matches!(
        err,
        ModelError::InvalidResponse(message)
            if message.contains("future_chat_kind")
    ));
}

// Covers: DeepSeek V4 thinking mode 400s unless same-model tool-call turns
// always carry reasoning_content, including present-but-empty; other models
// and foreign targets keep the field omitted so strict hosts never see it
// Owner: openai chat completions history conversion
#[test]
fn chat_handoff_injects_empty_reasoning_content_for_same_model_tool_turns() {
    let identity = crate::model::ModelIdentity::new(
        "opencode-go",
        "openai-chat-completions",
        "deepseek-v4-pro",
    );
    let tool_call = || {
        vec![ContentBlock::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: json!({"command": "pwd"}),
        })]
    };

    let same_model_tool_turn = Message::assistant(crate::model::AssistantMessage {
        content: tool_call(),
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: Vec::new(),
    });
    let converted = to_openai_message_for_target(&same_model_tool_turn, Some(&identity)).unwrap();
    assert_eq!(converted.reasoning_content.as_deref(), Some(""));

    let same_model_text_turn = Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: Vec::new(),
    });
    let converted = to_openai_message_for_target(&same_model_text_turn, Some(&identity)).unwrap();
    assert!(converted.reasoning_content.is_none());

    let non_deepseek =
        crate::model::ModelIdentity::new("opencode-go", "openai-chat-completions", "kimi-k2.7");
    let non_deepseek_tool_turn = Message::assistant(crate::model::AssistantMessage {
        content: tool_call(),
        provenance: Some(non_deepseek.clone()),
        reasoning_summary: None,
        provider_context: Vec::new(),
    });
    let converted =
        to_openai_message_for_target(&non_deepseek_tool_turn, Some(&non_deepseek)).unwrap();
    assert!(converted.reasoning_content.is_none());

    let foreign_target =
        crate::model::ModelIdentity::new("openrouter", "openai-chat-completions", "other");
    let foreign_tool_turn = Message::assistant(crate::model::AssistantMessage {
        content: tool_call(),
        provenance: Some(identity),
        reasoning_summary: None,
        provider_context: Vec::new(),
    });
    let converted =
        to_openai_message_for_target(&foreign_tool_turn, Some(&foreign_target)).unwrap();
    assert!(converted.reasoning_content.is_none());
}

// Covers: non-stream completions must capture reasoning_content for replay
// Owner: openai chat completions response conversion
#[test]
fn convert_openai_response_captures_reasoning_content() {
    let response: ChatResponse = serde_json::from_value(json!({
        "choices": [{
            "message": {
                "content": "done",
                "reasoning_content": "think first"
            }
        }]
    }))
    .unwrap();
    let finish = convert_openai_response(response, ChatToolCallPolicy::Strict).unwrap();
    assert_eq!(finish.reasoning_content.as_deref(), Some("think first"));
    assert!(matches!(
        finish.response,
        ModelResponse::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "done")
    ));
}

#[test]
fn codex_handoff_restores_replay_item_position() {
    let source = crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
    let message = Message::assistant(crate::model::AssistantMessage {
        content: vec![
            ContentBlock::Text("answer".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: json!({"command": "pwd"}),
            }),
        ],
        provenance: Some(source.clone()),
        reasoning_summary: None,
        provider_context: vec![crate::model::ProviderContextBlock {
            identity: source.clone(),
            kind: "openai_response_output_item".into(),
            position: Some(0),
            data: json!({"type": "reasoning", "encrypted_content": "signed"}),
        }],
    });

    let input = codex_input_items_for_target(&[message], &mut Vec::new(), Some(&source)).unwrap();

    assert_eq!(input[0]["type"], "reasoning");
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[2]["type"], "function_call");
}

// Covers: multiple positioned replay items must reconstruct the provider's
// original output order; stale original indexes inserted into the shorter
// lowered list strand reasoning items behind the tool call, which strict
// Responses gateways reject (opencode-go muse-spark tool loops: HTTP 500)
// Owner: OpenAI Responses history conversion
#[test]
fn codex_handoff_restores_original_order_for_multiple_replay_items() {
    let source = crate::model::ModelIdentity::new("opencode-go", "openai-responses", "muse-test");
    let reasoning = |position: usize, id: &str| crate::model::ProviderContextBlock {
        identity: source.clone(),
        kind: "openai_response_output_item".into(),
        position: Some(position),
        data: json!({"type": "reasoning", "id": id, "encrypted_content": "signed"}),
    };
    let tool_call = |id: &str| {
        ContentBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "fetch_content".into(),
            arguments: json!({"urls": ["https://example.com"]}),
        })
    };

    struct Case {
        name: &'static str,
        content: Vec<ContentBlock>,
        provider_context: Vec<crate::model::ProviderContextBlock>,
        // (json pointer key, expected value) per wire item, in order
        expected: Vec<(&'static str, &'static str)>,
    }
    let cases = [
        // Original output: reasoning(0), reasoning(1), function_call(2).
        Case {
            name: "two reasoning items before a lone tool call",
            content: vec![tool_call("call_1")],
            provider_context: vec![reasoning(0, "rs_1"), reasoning(1, "rs_2")],
            expected: vec![("id", "rs_1"), ("id", "rs_2"), ("call_id", "call_1")],
        },
        // Original output: reasoning(0), function_call(1), reasoning(2),
        // function_call(3) - parallel calls with interleaved reasoning.
        Case {
            name: "reasoning interleaved between parallel tool calls",
            content: vec![tool_call("call_1"), tool_call("call_2")],
            provider_context: vec![reasoning(0, "rs_1"), reasoning(2, "rs_2")],
            expected: vec![
                ("id", "rs_1"),
                ("call_id", "call_1"),
                ("id", "rs_2"),
                ("call_id", "call_2"),
            ],
        },
    ];

    for case in cases {
        let message = Message::assistant(crate::model::AssistantMessage {
            content: case.content,
            provenance: Some(source.clone()),
            reasoning_summary: None,
            provider_context: case.provider_context,
        });
        let input =
            codex_input_items_for_target(&[message], &mut Vec::new(), Some(&source)).unwrap();
        let order = input
            .iter()
            .map(|item| {
                case.expected
                    .iter()
                    .map(|(key, _)| *key)
                    .find_map(|key| item.get(key).and_then(|value| value.as_str()))
                    .expect("wire item carries an id")
            })
            .collect::<Vec<_>>();
        let expected = case
            .expected
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        assert_eq!(order, expected, "{}", case.name);
    }
}

#[test]
fn codex_remote_compaction_marker_replays_item_without_portable_text() {
    let source = crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
    let message = Message::assistant(
        crate::model::AssistantMessage {
            content: Vec::new(),
            provenance: Some(source.clone()),
            reasoning_summary: None,
            provider_context: vec![crate::model::ProviderContextBlock {
                identity: source.clone(),
                kind: "openai_response_output_item".into(),
                position: Some(0),
                data: json!({"type": "compaction", "encrypted_content": "blob"}),
            }],
        }
        .with_portable_fallback("portable summary"),
    );

    let exact = codex_input_items_for_target(
        std::slice::from_ref(&message),
        &mut Vec::new(),
        Some(&source),
    )
    .unwrap();
    let foreign = codex_input_items_for_target(
        std::slice::from_ref(&message),
        &mut Vec::new(),
        Some(&crate::model::ModelIdentity::new(
            "anthropic",
            "anthropic-messages",
            "claude-test",
        )),
    )
    .unwrap();

    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0]["type"], "compaction");
    assert_eq!(exact[0]["encrypted_content"], "blob");
    assert_eq!(foreign.len(), 1);
    assert_eq!(foreign[0]["role"], "assistant");
    assert!(foreign[0]["content"]
        .as_str()
        .unwrap()
        .contains("portable summary"));
}

#[test]
fn codex_handoff_replays_only_exact_model_context() {
    let source = crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
    let message = Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(source.clone()),
        reasoning_summary: Some("verified it".into()),
        provider_context: vec![crate::model::ProviderContextBlock {
            identity: source.clone(),
            kind: "openai_response_output_item".into(),
            position: None,
            data: json!({"type": "reasoning", "encrypted_content": "signed"}),
        }],
    });

    let exact = codex_input_items_for_target(
        std::slice::from_ref(&message),
        &mut Vec::new(),
        Some(&source),
    )
    .unwrap();
    let foreign = codex_input_items_for_target(
        std::slice::from_ref(&message),
        &mut Vec::new(),
        Some(&crate::model::ModelIdentity::new(
            "anthropic",
            "anthropic-messages",
            "claude-test",
        )),
    )
    .unwrap();

    assert!(exact
        .iter()
        .any(|item| item["encrypted_content"] == "signed"));
    assert!(!foreign
        .iter()
        .any(|item| item["encrypted_content"] == "signed"));
    assert!(foreign.iter().any(|item| {
        item.get("content")
            .and_then(|content| content.as_str())
            .is_some_and(|content| content.contains("<reasoning_summary>"))
    }));
}
