use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    AbortedAssistant, AssistantMessage, ContentBlock, Message, ModelIdentity, ModelUsage,
    PartialToolCall, ProviderContextBlock,
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
        portable_fallback: None,
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
