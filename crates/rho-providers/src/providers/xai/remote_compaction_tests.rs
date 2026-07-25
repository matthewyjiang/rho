use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::model::{Message, ModelRequest, ToolSpec};
use crate::reasoning::ReasoningLevel;

#[test]
fn compact_request_body_is_unary_without_tools() {
    let messages = [
        Message::System("be helpful".into()),
        Message::user_text("hello"),
    ];
    let tools = [ToolSpec {
        name: "read_file".into(),
        description: "read a file".into(),
        input_schema: json!({"type": "object"}),
    }];
    let profile = XaiReasoningProfile::exact([
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]);
    let body = build_compact_request_body(
        "xai",
        "grok-4.5",
        &profile,
        ModelRequest {
            messages: &messages,
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: Some("session-1"),
        },
    )
    .unwrap();

    assert_eq!(body["model"], "grok-4.5");
    assert_eq!(body["instructions"], "be helpful");
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["store"], false);
    assert_eq!(body["prompt_cache_key"], "session-1");
    assert_eq!(body["reasoning"], json!({"effort": "high"}));
    assert!(body.get("stream").is_none());
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
    // Compact does not need to request encrypted reasoning content.
    assert!(body.get("include").is_none());
}

#[test]
fn compact_request_body_works_for_oauth_identity() {
    let body = build_compact_request_body(
        "xai-oauth",
        "grok-4.5",
        &XaiReasoningProfile::not_configurable(),
        ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert_eq!(body["model"], "grok-4.5");
    assert_eq!(body["input"][0]["role"], "user");
    assert!(body.get("reasoning").is_none());
}
