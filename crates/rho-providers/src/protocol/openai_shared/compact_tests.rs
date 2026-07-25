use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::model::ContentBlock;

const NOTICE: &str = "server-side compaction handoff notice";

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
fn replacement_uses_server_output_users_and_compaction_marker() {
    let identity = ModelIdentity::new("openai", "openai-responses", "gpt-5.4");
    let retained_system_messages = vec![Message::System("system".into())];
    let output = vec![
        json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "recent user"}]
        }),
        json!({
            "type": "compaction",
            "id": "cmp_1",
            "encrypted_content": "blob"
        }),
    ];
    let replacement = replacement_from_compact_output(
        identity.clone(),
        &retained_system_messages,
        &output,
        NOTICE,
    )
    .unwrap();

    assert!(matches!(replacement[0], Message::System(_)));
    assert!(matches!(
        &replacement[1],
        Message::User(blocks) if matches!(
            blocks.as_slice(),
            [ContentBlock::Text(text)] if text == "recent user"
        )
    ));
    let Message::EnrichedAssistant(marker) = replacement.last().unwrap() else {
        panic!("expected compaction marker");
    };
    assert_eq!(marker.provenance.as_ref(), Some(&identity));
    assert!(marker.content.is_empty());
    assert!(marker
        .portable_fallback()
        .is_some_and(|text| text.contains("server-side")));
    let native_context = marker
        .provider_context
        .iter()
        .find(|block| block.kind == COMPACTION_OUTPUT_ITEM_KIND)
        .expect("compaction context");
    assert_eq!(native_context.data["encrypted_content"], "blob");
    assert!(!replacement
        .iter()
        .any(|message| matches!(message, Message::Assistant(_))));
}

#[test]
fn parse_compact_response_reads_usage() {
    let identity = ModelIdentity::new("openai-codex", "openai-responses", "gpt-5.4");
    let body = json!({
        "id": "resp_compact",
        "object": "response.compaction",
        "output": [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "hi"}]
            },
            {
                "type": "compaction",
                "encrypted_content": "blob"
            }
        ],
        "usage": {
            "input_tokens": 100,
            "output_tokens": 5,
            "total_tokens": 105
        }
    });
    let (messages, usage) = parse_compact_response(identity, &[], &body, NOTICE).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.output_tokens, Some(5));
    assert_eq!(usage.total_tokens, Some(105));
}

#[test]
fn parse_compact_response_malformed_output_is_invalid() {
    let identity = ModelIdentity::new("openai", "openai-responses", "gpt-5.4");
    let body = json!({
        "id": "resp_compact",
        "output": {
            "not": "an array"
        }
    });
    let error = parse_compact_response(identity, &[Message::System("sys".into())], &body, NOTICE)
        .expect_err("malformed compact output must fail");
    assert!(matches!(error, ModelError::InvalidResponse(_)));
}

#[test]
fn xai_single_compaction_item_replaces_history() {
    let identity = ModelIdentity::new("xai", "openai-responses", "grok-4.5");
    let body = json!({
        "id": "cmp_01",
        "object": "response.compaction",
        "output": [
            {
                "type": "compaction",
                "id": "cmp_01",
                "encrypted_content": "opaque-blob"
            }
        ],
        "usage": {
            "input_tokens": 12000,
            "output_tokens": 800,
            "total_tokens": 12800,
            "dropped_message_count": 45
        }
    });
    let (messages, usage) = parse_compact_response(
        identity.clone(),
        &[Message::System("tutor".into())],
        &body,
        NOTICE,
    )
    .unwrap();

    assert_eq!(messages.len(), 2);
    assert!(matches!(&messages[0], Message::System(text) if text == "tutor"));
    let Message::EnrichedAssistant(marker) = &messages[1] else {
        panic!("expected compaction marker");
    };
    assert_eq!(marker.provenance.as_ref(), Some(&identity));
    assert_eq!(
        marker.provider_context[0].data["encrypted_content"],
        "opaque-blob"
    );
    assert_eq!(usage.input_tokens, Some(12000));
    assert_eq!(usage.output_tokens, Some(800));
}
