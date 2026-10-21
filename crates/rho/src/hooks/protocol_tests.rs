use pretty_assertions::assert_eq;

use rho_sdk::hooks::HookDecision;

use super::*;

// Covers: only a well-formed, current-version decision is read as consent;
// every other handler output is a typed error so dispatch fails closed.
// Owner: hook decision protocol parser.
#[test]
fn parse_decision_accepts_only_well_formed_current_decisions() {
    let deny = |reason: &str| HookDecision::Deny {
        reason: reason.into(),
    };
    let cases: [(&str, &[u8], Result<HookDecision, DecisionError>); 10] = [
        (
            "continue",
            br#"{"version":1,"decision":"continue"}"#,
            Ok(HookDecision::Continue),
        ),
        (
            "deny with reason",
            br#"{"version":1,"decision":"deny","reason":"force push"}"#,
            Ok(deny("force push")),
        ),
        (
            "surrounding whitespace",
            b"\n  {\"version\":1,\"decision\":\"continue\"}  \n",
            Ok(HookDecision::Continue),
        ),
        (
            "unknown fields from newer handlers",
            br#"{"version":1,"decision":"continue","hint":"future"}"#,
            Ok(HookDecision::Continue),
        ),
        (
            "future schema version",
            br#"{"version":2,"decision":"continue"}"#,
            Err(DecisionError::SchemaMismatch { found: 2 }),
        ),
        ("empty output", b"", Err(DecisionError::Empty)),
        ("blank output", b"   \n", Err(DecisionError::Empty)),
        (
            "unknown decision word",
            br#"{"version":1,"decision":"allow"}"#,
            Err(DecisionError::UnknownDecision("allow".into())),
        ),
        (
            "deny without reason",
            br#"{"version":1,"decision":"deny"}"#,
            Err(DecisionError::MissingReason),
        ),
        (
            "deny with blank reason",
            br#"{"version":1,"decision":"deny","reason":"  "}"#,
            Err(DecisionError::MissingReason),
        ),
    ];
    for (case, output, expected) in cases {
        assert_eq!(parse_decision(output), expected, "{case}");
    }
}

// Covers: malformed hook output is reported as an error instead of being read
// as a decision
// Owner: hook protocol
#[test]
fn malformed_output_is_reported_rather_than_guessed() {
    for output in [&b"{not json"[..], &b"continue"[..], &[0xff, 0xfe, 0xfd][..]] {
        assert!(
            matches!(parse_decision(output), Err(DecisionError::Malformed(_))),
            "{output:?}"
        );
    }
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
    assert!(reason.len() <= MAX_REASON_BYTES);
    assert!(reason.chars().all(|character| character == '\u{00e9}'));
}
