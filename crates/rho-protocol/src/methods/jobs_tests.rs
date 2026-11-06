use pretty_assertions::assert_eq;

use super::*;

// Covers: a kind or state a newer daemon adds reads as `Unknown` instead of
// failing the whole job list, while known names keep their meaning.
// Owner: protocol forward compatibility.
#[test]
fn unknown_job_kinds_and_states_read_as_unknown() {
    for (json, kind) in [
        ("\"thread\"", JobKind::Thread),
        ("\"workflow\"", JobKind::Unknown),
    ] {
        assert_eq!(
            serde_json::from_str::<JobKind>(json).unwrap(),
            kind,
            "{json}"
        );
    }
    for (json, state) in [
        ("\"waiting_for_input\"", JobState::WaitingForInput),
        ("\"interrupted\"", JobState::Interrupted),
        ("\"queued\"", JobState::Unknown),
    ] {
        assert_eq!(
            serde_json::from_str::<JobState>(json).unwrap(),
            state,
            "{json}"
        );
    }
}
