use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::{
    protocol::anthropic_messages::AnthropicOutputConfig,
    provider_backend::{ContentBlock, Message, ToolCall, ToolSpec},
    reasoning::ReasoningLevel,
};

fn test_provider(model: &str) -> AnthropicProvider {
    let mut provider =
        AnthropicProvider::new(model.into(), "test-key".into(), |_| DEFAULT_MAX_TOKENS);
    provider.api_base = "https://example.test/v1".into();
    provider
}

fn test_provider_with_capabilities(
    model: &str,
    capabilities: &serde_json::Value,
) -> AnthropicProvider {
    test_provider(model).with_thinking_protocol(
        thinking::AnthropicThinkingProtocol::from_capabilities(model, capabilities),
    )
}

fn adaptive_capabilities() -> serde_json::Value {
    json!({
        "thinking": {
            "supported": true,
            "types": {
                "adaptive": {"supported": true},
                "enabled": {"supported": false}
            }
        },
        "effort": {
            "supported": true,
            "low": {"supported": true},
            "medium": {"supported": true},
            "high": {"supported": true},
            "xhigh": {"supported": true},
            "max": {"supported": true}
        }
    })
}

fn enabled_capabilities() -> serde_json::Value {
    json!({
        "thinking": {
            "supported": true,
            "types": {
                "adaptive": {"supported": false},
                "enabled": {"supported": true}
            }
        }
    })
}

fn request_body(
    provider: &AnthropicProvider,
    reasoning_level: ReasoningLevel,
) -> Result<AnthropicRequest, ModelError> {
    let messages = [Message::user_text("hello")];
    provider.request_body(
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level,
            prompt_cache_key: None,
        },
        false,
    )
}

#[test]
fn request_body_serializes_messages_tools_and_stream_flag() {
    let provider = test_provider_with_capabilities("claude-sonnet-4-5", &json!({}));
    let body = provider
        .request_body(
            ModelRequest {
                messages: &[
                    Message::System("system prompt".into()),
                    Message::User(vec![ContentBlock::Text("hello".into())]),
                    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                        id: "toolu_1".into(),
                        name: "bash".into(),
                        arguments: json!({"command":"pwd"}),
                    })]),
                ],
                tools: &[ToolSpec {
                    name: "bash".into(),
                    description: "run command".into(),
                    input_schema: json!({"type":"object"}),
                }],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: Some("ignored"),
            },
            true,
        )
        .unwrap();

    let value = serde_json::to_value(body).unwrap();
    assert_eq!(value["model"], "claude-sonnet-4-5");
    assert_eq!(value["max_tokens"], DEFAULT_MAX_TOKENS);
    assert_eq!(value["system"][0]["text"], "system prompt");
    assert_eq!(
        value["system"][0]["cache_control"],
        json!({"type":"ephemeral"})
    );
    assert_eq!(value["stream"], true);
    assert_eq!(value["tools"][0]["name"], "bash");
    assert_eq!(
        value["tools"][0]["cache_control"],
        json!({"type":"ephemeral"})
    );
    assert!(value.get("cache_control").is_none());
    assert!(value.get("prompt_cache_key").is_none());
    assert_eq!(value["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(
        value["messages"][0]["content"][0]["cache_control"],
        json!({"type":"ephemeral"})
    );
}

#[test]
fn adaptive_thinking_uses_output_effort_without_a_token_budget() {
    let provider = test_provider_with_capabilities("claude-opus-5", &adaptive_capabilities());

    let body = request_body(&provider, ReasoningLevel::Medium).unwrap();
    let value = serde_json::to_value(&body).unwrap();

    assert_eq!(body.max_tokens, DEFAULT_MAX_TOKENS);
    assert_eq!(
        body.thinking,
        Some(AnthropicThinkingConfig::Adaptive {
            display: "summarized"
        })
    );
    assert_eq!(
        body.output_config,
        Some(AnthropicOutputConfig { effort: "medium" })
    );
    assert_eq!(
        value["thinking"],
        json!({"type": "adaptive", "display": "summarized"})
    );
    assert_eq!(value["output_config"], json!({"effort": "medium"}));
    assert!(value["thinking"].get("budget_tokens").is_none());
}

#[test]
fn provider_context_replay_follows_effective_thinking_mode() {
    let adaptive = AnthropicThinkingConfig::Adaptive {
        display: "summarized",
    };
    let disabled = AnthropicThinkingConfig::Disabled;

    assert!(matches!(
        provider_context_replay(Some(&adaptive)),
        ProviderContextReplay::Enabled
    ));
    assert!(matches!(
        provider_context_replay(Some(&disabled)),
        ProviderContextReplay::Disabled
    ));
    assert!(matches!(
        provider_context_replay(None),
        ProviderContextReplay::Disabled
    ));
}

#[test]
fn reasoning_off_disables_adaptive_thinking_when_supported() {
    let mut capabilities = adaptive_capabilities();
    capabilities["thinking"]["types"]["disabled"] = json!({"supported": true});
    let provider = test_provider_with_capabilities("claude-opus-5", &capabilities);

    let body = request_body(&provider, ReasoningLevel::Off).unwrap();
    let value = serde_json::to_value(&body).unwrap();

    assert_eq!(body.thinking, Some(AnthropicThinkingConfig::Disabled));
    assert_eq!(body.output_config, None);
    assert_eq!(value["thinking"], json!({"type": "disabled"}));
}

#[test]
fn unknown_model_rejects_requested_reasoning_instead_of_omitting_thinking() {
    // An empty cache dir keeps the "no cached capabilities" precondition owned
    // by the test instead of by whatever else touched the shared test cache.
    let cache = tempfile::tempdir().unwrap();
    crate::model::provider_models::with_provider_models_cache_dir_for_tests(
        cache.path().to_path_buf(),
        || {
            let provider = test_provider("claude-opus-5");

            assert!(matches!(
                request_body(&provider, ReasoningLevel::Medium),
                Err(ModelError::InvalidResponse(_))
            ));
            let body = request_body(&provider, ReasoningLevel::Off).unwrap();
            assert_eq!(body.thinking, None);
            assert_eq!(body.output_config, None);
        },
    );
}

#[test]
fn legacy_thinking_still_reserves_answer_tokens() {
    let provider = test_provider_with_capabilities("claude-sonnet-4-5", &enabled_capabilities());

    let body = request_body(&provider, ReasoningLevel::Medium).unwrap();

    assert_eq!(body.max_tokens, DEFAULT_MAX_TOKENS);
    assert_eq!(
        body.thinking,
        Some(AnthropicThinkingConfig::Enabled {
            budget_tokens: DEFAULT_MAX_TOKENS - ANTHROPIC_ANSWER_RESERVE_TOKENS,
        })
    );
    assert_eq!(body.output_config, None);
}

// Covers: the cache write lands on the shared transcript; raising review
// reasoning changes thinking or effort, so that is not a guaranteed cache hit
// Owner: anthropic request body cache breakpoints
#[test]
fn two_stage_bodies_mark_shared_transcript_and_keep_screen_thinking_cheap() {
    let marker = Some(AnthropicCacheControl::ephemeral());
    let cases = [
        (
            "claude-opus-4-8",
            adaptive_capabilities(),
            Some(AnthropicThinkingConfig::Adaptive {
                display: "summarized",
            }),
            Some(AnthropicThinkingConfig::Adaptive {
                display: "summarized",
            }),
            Some(AnthropicOutputConfig { effort: "low" }),
            Some(AnthropicOutputConfig { effort: "high" }),
        ),
        (
            "claude-haiku-4-5",
            enabled_capabilities(),
            Some(AnthropicThinkingConfig::Enabled {
                budget_tokens: 2_048,
            }),
            Some(AnthropicThinkingConfig::Enabled {
                budget_tokens: DEFAULT_MAX_TOKENS.saturating_sub(ANTHROPIC_ANSWER_RESERVE_TOKENS),
            }),
            None,
            None,
        ),
    ];

    for (model, capabilities, screen_thinking, review_thinking, screen_effort, review_effort) in
        cases
    {
        let provider = test_provider_with_capabilities(model, &capabilities);
        let screen = two_stage_request_body(&provider, "screen", ReasoningLevel::Low);
        let review = two_stage_request_body(&provider, "review", ReasoningLevel::High);

        assert_eq!(screen.system, review.system, "{model} system");
        let screen_user = user_text_blocks(&screen);
        let review_user = user_text_blocks(&review);
        assert_eq!(
            screen_user[0],
            ("shared transcript", marker.as_ref()),
            "{model} screen transcript"
        );
        assert_eq!(
            review_user[0],
            ("shared transcript", marker.as_ref()),
            "{model} review transcript"
        );
        assert_eq!(screen_user[1], ("screen", None), "{model} screen suffix");
        assert_eq!(review_user[1], ("review", None), "{model} review suffix");
        assert_eq!(screen.thinking, screen_thinking, "{model} screen thinking");
        assert_eq!(review.thinking, review_thinking, "{model} review thinking");
        assert_eq!(screen.output_config, screen_effort, "{model} screen effort");
        assert_eq!(review.output_config, review_effort, "{model} review effort");
        assert!(
            screen.thinking != review.thinking || screen.output_config != review.output_config,
            "{model} raised review reasoning must change wire thinking or effort"
        );
    }
}

fn two_stage_request_body(
    provider: &AnthropicProvider,
    instruction: &str,
    reasoning_level: ReasoningLevel,
) -> AnthropicRequest {
    let messages = [
        Message::System("classifier".into()),
        Message::User(vec![
            ContentBlock::Text("shared transcript".into()),
            ContentBlock::Text(instruction.into()),
        ]),
    ];
    provider
        .request_body(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                reasoning_level,
                prompt_cache_key: None,
            },
            false,
        )
        .unwrap()
}

fn user_text_blocks(body: &AnthropicRequest) -> [(&str, Option<&AnthropicCacheControl>); 2] {
    match body.messages[0].content.as_slice() {
        [AnthropicContentBlock::Text {
            text: first,
            cache_control: first_cache,
        }, AnthropicContentBlock::Text {
            text: second,
            cache_control: second_cache,
        }] => [
            (first.as_str(), first_cache.as_ref()),
            (second.as_str(), second_cache.as_ref()),
        ],
        other => panic!("expected two user text blocks, got {other:?}"),
    }
}

#[test]
fn request_body_removes_top_level_schema_composition_from_tools() {
    let provider = test_provider_with_capabilities("claude-sonnet-4-5", &json!({}));
    let body = provider
        .request_body(
            ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[ToolSpec {
                    name: "edit".into(),
                    description: "edit files".into(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "value": {
                                "anyOf": [
                                    {"type": "string"},
                                    {"type": "null"}
                                ]
                            }
                        },
                        "anyOf": [
                            {"required": ["path"]},
                            {"required": ["value"]}
                        ],
                        "oneOf": [{"type": "object"}],
                        "allOf": [{"type": "object"}]
                    }),
                }],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
            false,
        )
        .unwrap();

    let value = serde_json::to_value(body).unwrap();
    let schema = &value["tools"][0]["input_schema"];
    assert!(schema.get("anyOf").is_none());
    assert!(schema.get("oneOf").is_none());
    assert!(schema.get("allOf").is_none());
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["value"].get("anyOf").is_some());
    assert_eq!(schema["properties"]["path"]["type"], "string");
}

#[test]
fn request_body_types_pure_composition_tool_schemas_for_anthropic() {
    let provider = test_provider_with_capabilities("claude-sonnet-5", &json!({}));
    let body = provider
        .request_body(
            ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[ToolSpec {
                    name: "workflow".into(),
                    description: "run workflows".into(),
                    input_schema: json!({
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "action": {"const": "validate"},
                                    "file": {"type": "string"}
                                },
                                "required": ["action", "file"]
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "action": {"const": "run"},
                                    "plan_id": {"type": "string"}
                                },
                                "required": ["action", "plan_id"]
                            }
                        ]
                    }),
                }],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
            false,
        )
        .unwrap();

    let value = serde_json::to_value(body).unwrap();
    let schema = &value["tools"][0]["input_schema"];
    assert!(schema.get("oneOf").is_none());
    assert!(schema.get("anyOf").is_none());
    assert!(schema.get("allOf").is_none());
    assert_eq!(schema["type"], "object");
}

#[test]
fn request_body_forces_non_object_root_schema_type_to_object() {
    let provider = test_provider_with_capabilities("claude-sonnet-5", &json!({}));
    let body = provider
        .request_body(
            ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[ToolSpec {
                    name: "odd".into(),
                    description: "odd root type".into(),
                    input_schema: json!({
                        "type": "string",
                        "properties": {
                            "path": {"type": "string"}
                        }
                    }),
                }],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
            false,
        )
        .unwrap();

    let value = serde_json::to_value(body).unwrap();
    let schema = &value["tools"][0]["input_schema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["path"]["type"], "string");
}
