use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

// Covers: Kimi /models reasoning metadata maps to canonical levels: valid
// efforts are deduplicated and gain Off, the default effort adds no level,
// non-reasoning models only offer Off, and missing or malformed efforts stay
// Unknown
// Owner: kimi model discovery
#[test]
fn reasoning_metadata_maps_to_canonical_levels() {
    use ReasoningLevel::{High, Low, Max, Off};

    let levels =
        |levels: Vec<ReasoningLevel>| ReasoningCapabilities::Levels(ReasoningLevelSet::new(levels));
    let cases = [
        (
            "efforts are canonical and include off",
            json!({
                "supports_reasoning": true,
                "supports_thinking_type": "only",
                "think_efforts": {
                    "support": true,
                    "valid_efforts": ["max", "low", "high", "low"],
                    "default_effort": "max"
                }
            }),
            levels(vec![Off, Low, High, Max]),
        ),
        (
            "default effort is not a selection level",
            json!({
                "supports_reasoning": true,
                "think_efforts": {
                    "support": true,
                    "valid_efforts": ["low"],
                    "default_effort": "max"
                }
            }),
            levels(vec![Off, Low]),
        ),
        (
            "non-reasoning model has only off",
            json!({"supports_reasoning": false}),
            levels(vec![Off]),
        ),
        (
            "missing efforts",
            json!({"supports_reasoning": true}),
            ReasoningCapabilities::Unknown,
        ),
        (
            "unsupported efforts",
            json!({
                "supports_reasoning": true,
                "think_efforts": {"support": false, "valid_efforts": ["low"]}
            }),
            ReasoningCapabilities::Unknown,
        ),
        (
            "empty efforts",
            json!({
                "supports_reasoning": true,
                "think_efforts": {"support": true, "valid_efforts": []}
            }),
            ReasoningCapabilities::Unknown,
        ),
        (
            "unknown effort",
            json!({
                "supports_reasoning": true,
                "think_efforts": {"support": true, "valid_efforts": ["turbo"]}
            }),
            ReasoningCapabilities::Unknown,
        ),
    ];

    for (case, value, expected) in cases {
        let metadata: KimiReasoningMetadata = serde_json::from_value(value).unwrap();
        assert_eq!(reasoning_capabilities(&metadata), expected, "{case}");
    }
}
