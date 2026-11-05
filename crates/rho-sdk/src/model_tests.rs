use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    AssistantMessage, ImageContent, InclusivePromptUsage, Message, ModelIdentity, ModelUsage,
    ProviderContextBlock,
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

// Covers: the producing adapter's replay scope decides which targets receive
// a block; provider and API boundaries always hold.
// Owner: sdk model
#[test]
fn replay_scope_bounds_native_replay_targets() {
    use super::ReplayScope;

    let source = ModelIdentity::new("openai-codex", "openai-responses", "source");
    let targets = [
        ModelIdentity::new("openai-codex", "openai-responses", "source"),
        ModelIdentity::new("openai-codex", "openai-responses", "target"),
        ModelIdentity::new("openai", "openai-responses", "source"),
        ModelIdentity::new("openai-codex", "another-api", "source"),
    ];
    let cases = [
        (ReplayScope::ExactModel, [true, false, false, false]),
        (ReplayScope::ProviderApi, [true, true, false, false]),
        (ReplayScope::Never, [false, false, false, false]),
    ];
    for (replay, expected) in cases {
        let block = ProviderContextBlock {
            identity: source.clone(),
            kind: "openai_response_output_item".into(),
            position: None,
            data: json!({"type": "compaction"}),
            replay,
        };
        let replayable = targets
            .iter()
            .map(|target| block.is_replayable_to(target))
            .collect::<Vec<_>>();
        assert_eq!(replayable, expected, "{replay:?}");
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
