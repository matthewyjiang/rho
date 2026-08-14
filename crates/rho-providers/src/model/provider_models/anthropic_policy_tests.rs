use serde_json::json;

use super::{
    capabilities_json, dated_parent_model, AnthropicModelCapabilities, AnthropicThinkingMode,
    OffThinking,
};

// Covers: a successful fetch always stores a capabilities object, including
// when the API omitted the field, so the row is known rather than cold cache,
// while projection stays Unknown instead of inventing NotConfigurable
// Owner: anthropic thinking protocol
#[test]
fn missing_or_non_object_capabilities_become_empty_object() {
    assert_eq!(capabilities_json(None), json!({}));
    assert_eq!(capabilities_json(Some(json!(null))), json!({}));
    assert_eq!(capabilities_json(Some(json!("adaptive"))), json!({}));
    assert_eq!(capabilities_json(Some(json!({}))), json!({}));
    assert!(
        matches!(
            AnthropicModelCapabilities::from_value(&capabilities_json(None))
                .unwrap()
                .thinking_mode("claude-haiku-4-5"),
            AnthropicThinkingMode::Unknown {
                off: OffThinking::Omit
            }
        ),
        "empty stored caps project to Unknown, not cold cache or NotConfigurable"
    );
}

#[test]
fn dated_parent_model_strips_yyyymmdd_suffix() {
    assert_eq!(
        dated_parent_model("claude-opus-5-20260724"),
        Some("claude-opus-5")
    );
    assert_eq!(dated_parent_model("claude-opus-5"), None);
    assert_eq!(dated_parent_model("claude-opus-5-preview"), None);
}
