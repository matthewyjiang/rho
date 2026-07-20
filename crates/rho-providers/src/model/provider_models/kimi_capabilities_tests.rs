use super::*;
use pretty_assertions::assert_eq;

fn parse(value: serde_json::Value) -> KimiReasoningMetadata {
    serde_json::from_value(value).unwrap()
}

#[test]
fn authenticated_efforts_are_canonical_and_include_off() {
    let metadata = parse(serde_json::json!({
        "supports_reasoning": true,
        "supports_thinking_type": "only",
        "think_efforts": {
            "support": true,
            "valid_efforts": ["max", "low", "high", "low"],
            "default_effort": "max"
        }
    }));

    assert_eq!(
        reasoning_capabilities(&metadata),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::High,
            ReasoningLevel::Max,
        ]))
    );
}

#[test]
fn default_effort_does_not_add_a_selection_level() {
    let metadata = parse(serde_json::json!({
        "supports_reasoning": true,
        "think_efforts": {
            "support": true,
            "valid_efforts": ["low"],
            "default_effort": "max"
        }
    }));

    assert_eq!(
        reasoning_capabilities(&metadata).levels(),
        Some([ReasoningLevel::Off, ReasoningLevel::Low].as_slice())
    );
}

#[test]
fn missing_or_malformed_efforts_are_unknown() {
    for value in [
        serde_json::json!({"supports_reasoning": true}),
        serde_json::json!({
            "supports_reasoning": true,
            "think_efforts": {"support": false, "valid_efforts": ["low"]}
        }),
        serde_json::json!({
            "supports_reasoning": true,
            "think_efforts": {"support": true, "valid_efforts": []}
        }),
        serde_json::json!({
            "supports_reasoning": true,
            "think_efforts": {"support": true, "valid_efforts": ["turbo"]}
        }),
    ] {
        assert_eq!(
            reasoning_capabilities(&parse(value)),
            ReasoningCapabilities::Unknown
        );
    }
}

#[test]
fn non_reasoning_model_has_only_off() {
    let metadata = parse(serde_json::json!({"supports_reasoning": false}));

    assert_eq!(
        reasoning_capabilities(&metadata).levels(),
        Some([ReasoningLevel::Off].as_slice())
    );
}
