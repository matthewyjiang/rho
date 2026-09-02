use serde_json::json;

use super::{
    capabilities_json, dated_parent_model, AnthropicModelCapabilities, AnthropicThinkingMode,
    OffThinking,
};
use crate::model::{ReasoningCapabilities, ReasoningLevelSet};
use crate::reasoning::ReasoningLevel;

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

// Covers: hosted Messages catalogs must map toggle and effort rows to the
// legacy budget-token surface instead of inferring adaptive thinking from
// generic effort levels
// Owner: anthropic thinking protocol
#[test]
fn host_catalog_capabilities_never_infer_adaptive_thinking() {
    assert_eq!(
        AnthropicThinkingMode::from_host_catalog_capabilities(ReasoningCapabilities::Unknown),
        AnthropicThinkingMode::BudgetTokens {
            off: OffThinking::Disabled
        }
    );
    assert_eq!(
        AnthropicThinkingMode::from_host_catalog_capabilities(ReasoningCapabilities::Levels(
            ReasoningLevelSet::new(vec![ReasoningLevel::Off, ReasoningLevel::Max])
        )),
        AnthropicThinkingMode::BudgetTokens {
            off: OffThinking::Disabled
        }
    );
    assert_eq!(
        AnthropicThinkingMode::from_host_catalog_capabilities(ReasoningCapabilities::Levels(
            ReasoningLevelSet::new(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::High
            ])
        )),
        AnthropicThinkingMode::BudgetTokens {
            off: OffThinking::Disabled
        }
    );
    assert_eq!(
        AnthropicThinkingMode::from_host_catalog_capabilities(ReasoningCapabilities::Levels(
            ReasoningLevelSet::new(vec![ReasoningLevel::Low, ReasoningLevel::High])
        )),
        AnthropicThinkingMode::BudgetTokens {
            off: OffThinking::Unsupported
        }
    );
}

// Covers: per-message effort is only advertised on families Anthropic
// documents; Fable 5 must not get the beta payload
// Owner: anthropic thinking protocol
#[test]
fn per_message_effort_is_limited_to_documented_model_families() {
    let cases = [
        ("claude-fable-5-1", true),
        ("claude-fable-5-1-20260901", true),
        ("claude-fable-5", false),
        ("claude-mythos-5-1", true),
        ("claude-mythos-5", false),
        ("claude-opus-5", true),
        ("claude-opus-5-20260724", true),
        ("claude-opus-4-8", false),
        ("claude-sonnet-5", false),
    ];
    for (model, supported) in cases {
        assert_eq!(
            super::supports_per_message_effort(model),
            supported,
            "{model}"
        );
    }
}
