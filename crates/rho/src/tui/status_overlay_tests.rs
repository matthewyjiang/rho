use super::{tone_for_message, StatusTone};
use pretty_assertions::assert_eq;

// Covers: status toast color must follow success / warning / error intent
// Owner: tui status surface
#[test]
fn tone_for_message_classifies_success_warning_and_error() {
    let cases = [
        ("interrupting tool", StatusTone::Warning),
        ("running", StatusTone::Warning),
        ("waiting for delegated agents", StatusTone::Warning),
        ("ready", StatusTone::Success),
        ("config saved", StatusTone::Success),
        ("attached image 1 (png)", StatusTone::Success),
        ("logout complete", StatusTone::Success),
        ("config save failed", StatusTone::Error),
        ("model switch rejected", StatusTone::Error),
        (
            "permission mode unavailable while running",
            StatusTone::Error,
        ),
        ("login failed", StatusTone::Error),
    ];
    for (message, tone) in cases {
        assert_eq!(tone_for_message(message), tone, "message={message}");
    }
}
