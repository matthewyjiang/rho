use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;

// Covers: a failed process card leads with outcome and command, and output
// containing its own fences stays verbatim instead of breaking the card body.
// Owner: process notification presentation.
#[test]
fn failed_process_card_shows_outcome_command_and_verbatim_output() {
    let card = ProcessNotification {
        process_id: "891ba4bc".into(),
        command: "cargo test".into(),
        state: State::Exited,
        exit_code: Some(101),
        output: "```rust\npanic!()\n```\n".into(),
        terminal_detail: None,
        elapsed: Duration::from_secs(65),
    }
    .card();

    assert_eq!(
        card,
        NotificationCard {
            title: "Failed (exit 101) · cargo test".into(),
            sender: "process".into(),
            recipient: "parent".into(),
            delivery: NotificationDelivery::Received,
            tone: NotificationTone::Error,
            preview: NotificationPreview::Truncated,
            visibility: NotificationVisibility::Conversation,
            reference: Some("891ba4bc".into()),
            subtitle: Some("ran 1m 05s".into()),
            body: "````text\n```rust\npanic!()\n```\n````".into(),
            details: vec![
                "process: 891ba4bc".into(),
                "command: cargo test".into(),
                "exit code: 101".into(),
            ],
        }
    );
}
