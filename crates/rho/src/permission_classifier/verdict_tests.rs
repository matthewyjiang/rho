use pretty_assertions::assert_eq;

use super::{parse_classifier_verdict, ClassifierVerdict, CLASSIFIER_PROMPT};

#[test]
fn parses_allow_and_deny_verdicts_from_json() {
    assert_eq!(
        parse_classifier_verdict(r#"{"decision":"allow"}"#).unwrap(),
        ClassifierVerdict::Allow
    );
    assert_eq!(
        parse_classifier_verdict(r#"{"decision":"deny","reason":"outside user intent"}"#).unwrap(),
        ClassifierVerdict::Deny {
            reason: "outside user intent".into()
        }
    );
}

#[test]
fn extracts_json_object_from_surrounding_prose() {
    assert_eq!(
        parse_classifier_verdict(
            "Here is my decision:\n```json\n{\"decision\":\"allow\"}\n```\nThanks."
        )
        .unwrap(),
        ClassifierVerdict::Allow
    );
    assert_eq!(
        parse_classifier_verdict(
            "Decision: {\"decision\":\"deny\",\"reason\":\"not requested by the user\"} done."
        )
        .unwrap(),
        ClassifierVerdict::Deny {
            reason: "not requested by the user".into()
        }
    );
}

#[test]
fn rejects_invalid_verdict_details() {
    assert!(parse_classifier_verdict("").is_err());
    assert!(parse_classifier_verdict(r#"{"decision":"deny","reason":"  "}"#).is_err());
    assert!(parse_classifier_verdict(r#"{"decision":"maybe","reason":"unclear"}"#).is_err());
    assert!(parse_classifier_verdict("no json here").is_err());
}

#[test]
fn classifier_prompt_requires_json_only_and_conservative_fail_closed() {
    assert!(CLASSIFIER_PROMPT.contains("JSON only"));
    assert!(CLASSIFIER_PROMPT.contains("fail closed"));
    assert!(CLASSIFIER_PROMPT.contains("rationalizations"));
    assert!(CLASSIFIER_PROMPT.contains(r#"{"decision":"allow"}"#));
    assert!(CLASSIFIER_PROMPT.contains(r#"{"decision":"deny","reason":"..."}"#));
}
