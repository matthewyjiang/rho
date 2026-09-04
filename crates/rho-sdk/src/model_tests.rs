use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    handoff::{prepare_assistant, report_omissions},
    AbortedAssistant, AssistantMessage, ContentBlock, ImageContent, InclusivePromptUsage, Message,
    ModelIdentity, ModelUsage, PartialToolCall, ProviderContextBlock,
};

#[test]
fn legacy_assistant_history_keeps_existing_json_shape() {
    let message = Message::assistant_text("hello");

    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        json!({"Assistant": [{"Text": "hello"}]})
    );
    assert_eq!(
        serde_json::from_value::<Message>(json!({"Assistant": [{"Text": "hello"}]})).unwrap(),
        message
    );
}

#[test]
fn enriched_assistant_history_round_trips_provider_context() {
    let message = Message::assistant(AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(ModelIdentity::new("openai", "responses", "gpt-5")),
        reasoning_summary: Some("summary".into()),
        provider_context: vec![ProviderContextBlock {
            identity: ModelIdentity::new("openai", "responses", "gpt-5"),
            kind: "reasoning".into(),
            position: Some(0),
            data: json!({"id": "item-1"}),
        }],
    });

    let encoded = serde_json::to_string(&message).unwrap();

    assert_eq!(serde_json::from_str::<Message>(&encoded).unwrap(), message);
}

#[test]
fn portable_fallback_round_trips_and_migrates_legacy_json() {
    let identity = ModelIdentity::new("openai", "responses", "gpt-5");
    let message = AssistantMessage {
        provenance: Some(identity.clone()),
        ..AssistantMessage::default()
    }
    .with_portable_fallback("portable notice");

    let encoded = serde_json::to_value(&message).unwrap();
    let round_trip = serde_json::from_value::<AssistantMessage>(encoded).unwrap();
    assert_eq!(round_trip, message);
    assert_eq!(round_trip.portable_fallback(), Some("portable notice"));
    assert!(!round_trip.provider_context[0].is_replayable_to(&identity));

    let migrated = serde_json::from_value::<AssistantMessage>(json!({
        "content": [],
        "provenance": identity,
        "reasoning_summary": null,
        "portable_fallback": "legacy notice",
        "provider_context": []
    }))
    .unwrap();
    assert_eq!(migrated.portable_fallback(), Some("legacy notice"));
}

#[test]
fn malformed_portable_metadata_remains_provider_context() {
    let identity = ModelIdentity::new("openai", "responses", "gpt-5");
    let mut message = AssistantMessage {
        provenance: Some(identity.clone()),
        ..AssistantMessage::default()
    }
    .with_portable_fallback("portable notice");
    message.provider_context[0].data = json!({"not": "text"});

    assert_eq!(message.portable_fallback(), None);
    assert!(message.provider_context[0].is_replayable_to(&identity));
}

#[test]
fn aborted_assistant_history_keeps_partial_tool_calls_and_usage() {
    let message = Message::AbortedAssistant(Box::new(AbortedAssistant {
        content: vec![ContentBlock::Text("partial".into())],
        reasoning: "ephemeral reasoning".into(),
        provenance: None,
        reasoning_summary: None,
        provider_context: Vec::new(),
        tool_calls: vec![PartialToolCall {
            id: Some("call-1".into()),
            name: Some("read_file".into()),
            arguments: "{\"path\":".into(),
        }],
        usage: ModelUsage {
            output_tokens: Some(4),
            ..ModelUsage::default()
        },
    }));

    let encoded = serde_json::to_string(&message).unwrap();

    assert_eq!(serde_json::from_str::<Message>(&encoded).unwrap(), message);
}

#[test]
fn provider_context_replays_only_to_exact_identity() {
    let block = ProviderContextBlock {
        identity: ModelIdentity::new("openai", "responses", "gpt-5"),
        kind: "reasoning".into(),
        position: None,
        data: json!({}),
    };

    assert!(block.is_replayable_to(&ModelIdentity::new("openai", "responses", "gpt-5")));
    assert!(!block.is_replayable_to(&ModelIdentity::new("openai", "responses", "gpt-5-mini")));
}

// Covers: async-call markers round-trip the call id, never replay as provider
// context, and never count as a handoff omission.
// Owner: sdk model
#[test]
fn async_tool_call_marker_round_trips_and_is_not_a_handoff_omission() {
    let identity = ModelIdentity::new("openai", "responses", "gpt-6-astra");
    let foreign = ModelIdentity::new("anthropic", "anthropic-messages", "claude-test");
    let cases = [("same identity", &identity), ("foreign identity", &foreign)];

    for (name, target) in cases {
        let block = ProviderContextBlock::async_tool_call(identity.clone(), "call-a");
        assert_eq!(block.async_tool_call_id(), Some("call-a"), "{name}");
        assert!(!block.is_replayable_to(target), "{name}");
        assert!(block.is_sdk_metadata(), "{name}");

        let message = AssistantMessage {
            content: vec![ContentBlock::Text("answer".into())],
            provenance: Some(identity.clone()),
            reasoning_summary: Some("should stay off content".into()),
            provider_context: vec![block],
        };
        let report = report_omissions(std::iter::once(&message), target);
        assert_eq!(report.omitted_provider_context, 0, "{name}");
        assert!(report.omitted_kinds.is_empty(), "{name}");

        let prepared = prepare_assistant(message, target);
        assert!(prepared.replay_context.is_empty(), "{name}");
        assert_eq!(
            prepared.content,
            vec![ContentBlock::Text("answer".into())],
            "{name}"
        );
    }
}

// Covers: image payloads are typed from magic bytes; unknown bytes stay untyped.
// Owner: sdk model
#[test]
fn image_content_recognizes_supported_signatures() {
    assert_eq!(
        ImageContent::mime_type_from_bytes(b"\x89PNG\r\n\x1a\nrest"),
        Some("image/png")
    );
    assert_eq!(
        ImageContent::mime_type_from_bytes(b"\xff\xd8\xffrest"),
        Some("image/jpeg")
    );
    assert_eq!(
        ImageContent::mime_type_from_bytes(b"GIF89arest"),
        Some("image/gif")
    );
    assert_eq!(
        ImageContent::mime_type_from_bytes(b"RIFFxxxxWEBP"),
        Some("image/webp")
    );
    assert_eq!(ImageContent::mime_type_from_bytes(b"plain text"), None);
}

// Covers: inclusive-prompt hosts must not store mixed totals as uncached input,
// and later cache-split snapshots must not erase earlier mute prompt size
// Owner: sdk model usage
#[test]
fn inclusive_prompt_recovers_mute_totals_without_claiming_uncached() {
    let mute = ModelUsage::from_inclusive_prompt(InclusivePromptUsage {
        prompt_tokens: Some(100),
        output_tokens: Some(5),
        reported_total: Some(105),
        ..InclusivePromptUsage::default()
    });
    let explicit_zero_cache = ModelUsage::from_inclusive_prompt(InclusivePromptUsage {
        prompt_tokens: Some(100),
        output_tokens: Some(5),
        cache_read_tokens: Some(0),
        reported_total: Some(105),
        ..InclusivePromptUsage::default()
    });
    let split = ModelUsage::from_inclusive_prompt(InclusivePromptUsage {
        prompt_tokens: Some(100),
        output_tokens: Some(10),
        cache_read_tokens: Some(20),
        reported_total: Some(110),
        ..InclusivePromptUsage::default()
    });
    let growing_total = ModelUsage {
        total_tokens: Some(105),
        ..ModelUsage::default()
    };
    let merged = mute.saturating_add(&split);

    assert_eq!(
        mute,
        ModelUsage {
            output_tokens: Some(5),
            total_tokens: Some(105),
            ..ModelUsage::default()
        }
    );
    assert_eq!(mute.total_input_tokens(), None);
    assert_eq!(mute.inclusive_prompt_tokens(), Some(100));

    assert_eq!(explicit_zero_cache.input_tokens, Some(100));
    assert_eq!(explicit_zero_cache.cache_read_tokens, Some(0));
    assert_eq!(explicit_zero_cache.inclusive_prompt_tokens(), Some(100));

    assert_eq!(split.input_tokens, Some(80));
    assert_eq!(split.total_input_tokens(), Some(100));
    assert_eq!(split.inclusive_prompt_tokens(), Some(100));

    assert_eq!(growing_total.inclusive_prompt_tokens(), None);

    assert_eq!(merged.input_tokens, Some(80));
    assert_eq!(merged.cache_read_tokens, Some(20));
    assert_eq!(merged.total_input_tokens(), Some(100));
    assert_eq!(merged.inclusive_prompt_tokens(), Some(200));
}
