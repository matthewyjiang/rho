use super::{should_toast, tone_for_message, StatusTone};
use pretty_assertions::assert_eq;

// Covers: status toast color must follow success / warning / error intent
// Owner: tui status surface
#[test]
fn tone_for_message_classifies_success_warning_and_error() {
    let cases = [
        ("interrupting tool", StatusTone::Warning),
        ("running", StatusTone::Warning),
        ("waiting for delegated agents", StatusTone::Warning),
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

// Covers: routine progress and UI chrome must not open a toast
// Owner: tui status surface
#[test]
fn routine_status_does_not_toast() {
    let silent = [
        "ready",
        "running",
        "config",
        "running step 1",
        "running bash",
        "select model",
        "edit compact threshold percent",
        "confirm delete",
        "compacting context",
        "retrying provider response",
        "approval requested",
        "waiting for delegated agents",
        "Keyboard shortcuts",
        "Config · saves automatically",
        "loading models",
        "extracting report.pdf",
        "starting plan.rho",
        "opening a herdr pane for agent run-1",
    ];
    for message in silent {
        assert!(!should_toast(message), "should stay silent: {message}");
    }

    let toasted = [
        "interrupting tool",
        "config saved",
        "model switch failed",
        "permission mode: ask",
        "attached image 1 (png)",
        "new session",
        "unknown command",
        "inline shell: bash",
    ];
    for message in toasted {
        assert!(should_toast(message), "should toast: {message}");
    }
}
