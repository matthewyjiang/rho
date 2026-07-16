use std::str::FromStr;

use pretty_assertions::assert_eq;

use super::{Revision, RunId, SessionId, SteeringId, ToolCallId};

#[test]
fn opaque_ids_round_trip_through_strings_and_json() {
    let ids = [
        SessionId::from_str("session-1").unwrap().to_string(),
        RunId::from_str("run-1").unwrap().to_string(),
        SteeringId::from_str("steering-1").unwrap().to_string(),
        ToolCallId::from_str("call-1").unwrap().to_string(),
    ];

    assert_eq!(ids, ["session-1", "run-1", "steering-1", "call-1"]);
    assert_eq!(
        serde_json::to_string(&SessionId::from_str("session-1").unwrap()).unwrap(),
        r#""session-1""#,
    );
}

#[test]
fn opaque_ids_reject_empty_values() {
    assert_eq!(
        SessionId::from_str("").unwrap_err().to_string(),
        "identifier must not be empty"
    );
    assert!(serde_json::from_str::<SessionId>(r#""""#).is_err());
}

#[test]
fn revisions_advance_without_wrapping() {
    assert_eq!(
        Revision::INITIAL.checked_next(),
        Some(Revision::from_u64(1))
    );
    assert_eq!(Revision::from_u64(u64::MAX).checked_next(), None);
}
