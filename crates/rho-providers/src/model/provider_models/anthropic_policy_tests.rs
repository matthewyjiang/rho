use serde_json::{json, Value};

use crate::{
    model::{ReasoningCapabilities, ReasoningLevelSet},
    reasoning::ReasoningLevel,
};

use super::{
    capabilities_json, dated_parent_model, AnthropicModelCapabilities, AnthropicThinkingMode,
    OffThinking,
};

// Covers: selectable reasoning levels must match the wire protocol the model
// advertises, so pickers never offer an effort Anthropic rejects
// Owner: anthropic thinking protocol
#[test]
fn advertised_capabilities_decide_selectable_reasoning_levels() {
    let adaptive_with_effort = json!({
        "thinking": {"types": {"adaptive": {"supported": true}}},
        "effort": {
            "supported": true,
            "low": {"supported": true},
            "medium": {"supported": true},
            "high": {"supported": true},
            "max": {"supported": true}
        }
    });
    let cases: [(&str, &str, &Value, ReasoningCapabilities); 5] = [
        (
            "adaptive effort models drop minimal and keep off",
            "claude-opus-5",
            &adaptive_with_effort,
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ])),
        ),
        (
            "a model that cannot disable thinking has no off",
            "claude-mythos-5",
            &adaptive_with_effort,
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ])),
        ),
        (
            "budget-token models accept the whole ladder",
            "claude-sonnet-4-5",
            &json!({
                "thinking": {"types": {
                    "enabled": {"supported": true},
                    "disabled": {"supported": true}
                }}
            }),
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Minimal,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Xhigh,
                ReasoningLevel::Max,
            ])),
        ),
        (
            "adaptive without an effort control is not configurable",
            "claude-opus-5",
            &json!({"thinking": {"types": {"adaptive": {"supported": true}}}}),
            ReasoningCapabilities::NotConfigurable,
        ),
        (
            "a row advertising no thinking type is not configurable",
            "claude-haiku-4-5",
            &json!({}),
            ReasoningCapabilities::NotConfigurable,
        ),
    ];

    for (name, model, capabilities, expected) in cases {
        let parsed = AnthropicModelCapabilities::from_value(capabilities).unwrap();
        assert_eq!(parsed.reasoning_capabilities(model), expected, "{name}");
    }
}

// Covers: a successful fetch always stores a capabilities object, including
// when the API omitted the field, so the row is known NoControl rather than cold
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
            AnthropicThinkingMode::NoControl { off: OffThinking::Omit }
        ),
        "empty stored caps project to NoControl, not cold cache"
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
