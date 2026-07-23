use super::*;
use crate::model::ContentBlock;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn supports_openai_codex_and_api_key_openai() {
    assert!(supports_remote_compaction(&ModelIdentity::new(
        "openai-codex",
        "openai-responses",
        "gpt-5.4",
    )));
    assert!(supports_remote_compaction(&ModelIdentity::new(
        "openai",
        "openai-chat-completions",
        "gpt-5.4",
    )));
    assert!(!supports_remote_compaction(&ModelIdentity::new(
        "anthropic",
        "anthropic-messages",
        "claude",
    )));
}

#[test]
fn remote_compaction_body_appends_trigger_and_include_for_api_key() {
    let identity = ModelIdentity::new("openai", "openai-chat-completions", "gpt-5.4");
    let body = build_remote_compaction_body(
        &identity,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &[
                Message::System("be helpful".into()),
                Message::user_text("hello"),
            ],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: Some("session-1"),
        },
    )
    .unwrap();

    let input = body["input"].as_array().unwrap();
    assert_eq!(input.last().unwrap()["type"], "compaction_trigger");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["prompt_cache_key"], "session-1");
    assert_eq!(body["parallel_tool_calls"], true);
    assert!(body.get("tools").is_none());
}

#[test]
fn remote_compaction_body_keeps_codex_responses_lite_shape() {
    let identity = ModelIdentity::new("openai-codex", "openai-responses", "gpt-5.6-sol");
    let body = build_remote_compaction_body(
        &identity,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .unwrap();

    assert_eq!(
        body["input"].as_array().unwrap().last().unwrap()["type"],
        "compaction_trigger"
    );
    assert_eq!(
        body["reasoning"],
        json!({"effort": "medium", "summary": "auto", "context": "all_turns"})
    );
}

#[test]
fn extract_compaction_item_requires_exactly_one_valid_item() {
    assert!(extract_compaction_item(&[]).is_err());
    assert!(extract_compaction_item(&[json!({"type": "message"})]).is_err());
    assert!(extract_compaction_item(&[json!({
        "type": "compaction",
        "encrypted_content": ""
    })])
    .is_err());
    assert!(extract_compaction_item(&[
        json!({"type": "compaction", "encrypted_content": "a"}),
        json!({"type": "compaction", "encrypted_content": "b"}),
    ])
    .is_err());

    let item = extract_compaction_item(&[
        json!({"type": "reasoning", "encrypted_content": "r"}),
        json!({"type": "compaction", "encrypted_content": "blob"}),
    ])
    .unwrap();
    assert_eq!(item["encrypted_content"], "blob");
}

#[test]
fn replacement_keeps_systems_recent_users_and_compaction_marker() {
    let identity = ModelIdentity::new("openai", "openai-chat-completions", "gpt-5.4");
    let messages = vec![
        Message::System("system".into()),
        Message::user_text("old user"),
        Message::assistant_text("old assistant"),
        Message::user_text("recent user"),
        Message::assistant_text("recent assistant"),
    ];
    let replacement = build_remote_compaction_replacement(
        identity.clone(),
        &messages,
        json!({"type": "compaction", "encrypted_content": "blob"}),
        Some("portable summary".into()),
    )
    .unwrap();

    assert!(matches!(replacement[0], Message::System(_)));
    let Message::EnrichedAssistant(marker) = replacement.last().unwrap() else {
        panic!("expected compaction marker");
    };
    assert_eq!(marker.provenance.as_ref(), Some(&identity));
    assert_eq!(
        marker.content,
        vec![ContentBlock::Text("portable summary".into())]
    );
    assert_eq!(marker.provider_context.len(), 1);
    assert_eq!(marker.provider_context[0].kind, COMPACTION_OUTPUT_ITEM_KIND);
    assert_eq!(marker.provider_context[0].data["encrypted_content"], "blob");
    assert!(replacement.iter().any(|message| matches!(
        message,
        Message::User(blocks) if matches!(
            blocks.as_slice(),
            [ContentBlock::Text(text)] if text == "recent user"
        )
    )));
    assert!(!replacement
        .iter()
        .any(|message| matches!(message, Message::Assistant(_))));
    assert!(history_has_remote_compaction(&replacement, &identity));
}

#[test]
fn retain_recent_users_respects_token_budget() {
    let messages = vec![
        Message::user_text("a".repeat(40)),
        Message::user_text("b".repeat(40)),
        Message::user_text("c".repeat(8)),
    ];
    let retained = retain_recent_user_messages(messages, 5);
    assert_eq!(retained.len(), 1);
    let Message::User(blocks) = &retained[0] else {
        panic!("expected user");
    };
    let ContentBlock::Text(text) = &blocks[0] else {
        panic!("expected text");
    };
    assert!(text.starts_with('c') || text.starts_with('b'));
}
