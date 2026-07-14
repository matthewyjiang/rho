use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::provider_backend::{ContentBlock, Message, ToolCall, ToolSpec};

fn test_provider(model: &str) -> AnthropicProvider {
    AnthropicProvider {
        client: provider_client(),
        api_key: "test-key".into(),
        api_base: "https://example.test/v1".into(),
        model: model.into(),
        max_tokens: |_| DEFAULT_MAX_TOKENS,
        reasoning: ReasoningLevel::Off,
    }
}

fn request_body(provider: &AnthropicProvider) -> AnthropicRequest {
    let messages = [Message::user_text("hello")];
    provider
        .request_body(
            ModelRequest {
                messages: &messages,
                tools: &[],
                cancellation: Default::default(),
                prompt_cache_key: None,
            },
            false,
        )
        .unwrap()
}

#[test]
fn request_body_serializes_messages_tools_and_stream_flag() {
    let provider = test_provider("claude-sonnet-4-5");
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
}

#[test]
fn adaptive_thinking_uses_output_effort_without_a_token_budget() {
    let mut provider = test_provider("claude-opus-4-8");
    provider.set_reasoning(ReasoningLevel::Medium);

    let body = request_body(&provider);
    let value = serde_json::to_value(&body).unwrap();

    assert_eq!(body.max_tokens, DEFAULT_MAX_TOKENS);
    assert_eq!(body.thinking, Some(AnthropicThinkingConfig::Adaptive));
    assert_eq!(
        body.output_config,
        Some(AnthropicOutputConfig { effort: "medium" })
    );
    assert_eq!(value["thinking"], json!({"type": "adaptive"}));
    assert_eq!(value["output_config"], json!({"effort": "medium"}));
    assert!(value["thinking"].get("budget_tokens").is_none());
}

#[test]
fn legacy_thinking_still_reserves_answer_tokens() {
    let mut provider = test_provider("claude-sonnet-4-5");
    provider.set_reasoning(ReasoningLevel::Medium);

    let body = request_body(&provider);

    assert_eq!(body.max_tokens, DEFAULT_MAX_TOKENS);
    assert_eq!(
        body.thinking,
        Some(AnthropicThinkingConfig::Enabled {
            budget_tokens: DEFAULT_MAX_TOKENS - ANTHROPIC_ANSWER_RESERVE_TOKENS,
        })
    );
    assert_eq!(body.output_config, None);
}

#[test]
fn adaptive_effort_uses_only_levels_supported_by_each_model() {
    assert_eq!(
        adaptive_effort("claude-opus-4-6", ReasoningLevel::Minimal),
        "low"
    );
    assert_eq!(
        adaptive_effort("claude-opus-4-6", ReasoningLevel::Xhigh),
        "high"
    );
    assert_eq!(
        adaptive_effort("claude-opus-4-8-20260401", ReasoningLevel::Xhigh),
        "xhigh"
    );
}

#[test]
fn request_body_removes_top_level_schema_composition_from_tools() {
    let provider = test_provider("claude-sonnet-4-5");
    let body = provider
        .request_body(
            ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[ToolSpec {
                    name: "edit_file".into(),
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
    assert!(schema["properties"]["value"].get("anyOf").is_some());
    assert_eq!(schema["properties"]["path"]["type"], "string");
}
