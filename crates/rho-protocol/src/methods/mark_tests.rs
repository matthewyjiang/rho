use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: each mark kind travels under its own word, the mark a client
// sets naming no time but a snooze's end, and a clear leaving `mark` out
// both ways, so every frontend of one daemon reads one shape.
// Owner: protocol wire contract.
#[test]
fn a_mark_round_trips_through_its_wire_form() {
    let session = || SessionId::new("s1");
    let cases = [
        (
            Some(NewSessionMark::Done),
            json!({"session": "s1", "mark": {"kind": "done"}}),
            Some(SessionMark::Done { at: 1_792_133_984 }),
            json!({"mark": {"kind": "done", "at": 1_792_133_984}}),
        ),
        (
            Some(NewSessionMark::Snoozed {
                until: 1_792_200_000,
            }),
            json!({"session": "s1", "mark": {"kind": "snoozed", "until": 1_792_200_000}}),
            Some(SessionMark::Snoozed {
                until: 1_792_200_000,
            }),
            json!({"mark": {"kind": "snoozed", "until": 1_792_200_000}}),
        ),
        (
            Some(NewSessionMark::Pinned),
            json!({"session": "s1", "mark": {"kind": "pinned"}}),
            Some(SessionMark::Pinned { at: 1_792_133_984 }),
            json!({"mark": {"kind": "pinned", "at": 1_792_133_984}}),
        ),
        (
            Some(NewSessionMark::Kept),
            json!({"session": "s1", "mark": {"kind": "kept"}}),
            Some(SessionMark::Kept { at: 1_792_133_984 }),
            json!({"mark": {"kind": "kept", "at": 1_792_133_984}}),
        ),
        (None, json!({"session": "s1"}), None, json!({})),
    ];
    for (set, params_wire, saved, response_wire) in cases {
        let params = SessionMarkParams::new(session(), set);
        let response = SessionMarkResponse::new(saved);
        assert_eq!(
            (
                serde_json::to_value(&params).unwrap(),
                serde_json::to_value(&response).unwrap(),
            ),
            (params_wire.clone(), response_wire.clone())
        );
        assert_eq!(
            (
                serde_json::from_value::<SessionMarkParams>(params_wire).unwrap(),
                serde_json::from_value::<SessionMarkResponse>(response_wire).unwrap(),
            ),
            (params, response)
        );
    }
}

// Covers: a kind the daemon does not know is refused when a client asks to
// set it, rather than saved as something else. Owner: protocol wire
// contract.
#[test]
fn a_kind_the_daemon_does_not_know_is_refused_from_a_client() {
    let newer = json!({"kind": "archived", "at": 1_792_133_984});
    assert_eq!(serde_json::from_value::<NewSessionMark>(newer).ok(), None);
}
