use pretty_assertions::assert_eq;

use rho_sdk::hooks::HookDecision;

use super::*;

#[test]
fn a_versioned_continue_parses() {
    assert_eq!(
        parse_decision(br#"{"version":1,"decision":"continue"}"#),
        Ok(HookDecision::Continue)
    );
}

#[test]
fn a_versioned_deny_carries_its_reason() {
    assert_eq!(
        parse_decision(br#"{"version":1,"decision":"deny","reason":"force push"}"#),
        Ok(HookDecision::Deny {
            reason: "force push".into()
        })
    );
}

#[test]
fn surrounding_whitespace_is_tolerated() {
    assert_eq!(
        parse_decision(b"\n  {\"version\":1,\"decision\":\"continue\"}  \n"),
        Ok(HookDecision::Continue)
    );
}

#[test]
fn unknown_fields_are_ignored_so_newer_handlers_keep_working() {
    assert_eq!(
        parse_decision(br#"{"version":1,"decision":"continue","hint":"future"}"#),
        Ok(HookDecision::Continue)
    );
}

#[test]
fn a_future_schema_version_is_not_read_as_consent() {
    assert_eq!(
        parse_decision(br#"{"version":2,"decision":"continue"}"#),
        Err(DecisionError::SchemaMismatch { found: 2 })
    );
}

#[test]
fn empty_output_is_not_read_as_consent() {
    assert_eq!(parse_decision(b""), Err(DecisionError::Empty));
    assert_eq!(parse_decision(b"   \n"), Err(DecisionError::Empty));
}

#[test]
fn malformed_json_is_reported_rather_than_guessed() {
    assert!(matches!(
        parse_decision(b"{not json"),
        Err(DecisionError::Malformed(_))
    ));
    assert!(matches!(
        parse_decision(b"continue"),
        Err(DecisionError::Malformed(_))
    ));
}

#[test]
fn an_unknown_decision_word_is_rejected() {
    assert_eq!(
        parse_decision(br#"{"version":1,"decision":"allow"}"#),
        Err(DecisionError::UnknownDecision("allow".into()))
    );
}

#[test]
fn a_denial_without_a_reason_is_rejected() {
    assert_eq!(
        parse_decision(br#"{"version":1,"decision":"deny"}"#),
        Err(DecisionError::MissingReason)
    );
    assert_eq!(
        parse_decision(br#"{"version":1,"decision":"deny","reason":"  "}"#),
        Err(DecisionError::MissingReason)
    );
}

#[test]
fn oversized_output_is_refused_before_parsing() {
    let payload = vec![b'x'; MAX_DECISION_BYTES + 1];

    assert_eq!(parse_decision(&payload), Err(DecisionError::TooLarge));
}

#[test]
fn a_long_denial_reason_is_shortened_on_a_character_boundary() {
    let reason = "\u{00e9}".repeat(2000);
    let payload = serde_json::json!({"version": 1, "decision": "deny", "reason": reason});

    let decision = parse_decision(payload.to_string().as_bytes()).unwrap();

    let HookDecision::Deny { reason } = decision else {
        panic!("expected a denial");
    };
    assert!(reason.len() <= 1024);
    assert!(reason.chars().all(|character| character == '\u{00e9}'));
}

#[test]
fn invalid_utf8_does_not_panic_and_does_not_parse() {
    assert!(matches!(
        parse_decision(&[0xff, 0xfe, 0xfd]),
        Err(DecisionError::Malformed(_))
    ));
}
