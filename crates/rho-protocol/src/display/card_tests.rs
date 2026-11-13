use pretty_assertions::assert_eq;

use super::*;

// Covers: ToolFact serde roundtrip keeps the timeout budget structured so
// hosts can decorate it live without parsing display text.
// Owner: pure unit (tool card)
#[test]
fn tool_fact_timeout_serde_roundtrip() {
    let cases = [
        (
            ToolFact::Timeout { seconds: Some(30) },
            r#"{"kind":"timeout","seconds":30}"#,
        ),
        (ToolFact::Timeout { seconds: None }, r#"{"kind":"timeout"}"#),
    ];
    for (fact, expected_json) in cases {
        let json = serde_json::to_string(&fact).unwrap();
        assert_eq!(json, expected_json);
        let decoded: ToolFact = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, fact);
    }
}
