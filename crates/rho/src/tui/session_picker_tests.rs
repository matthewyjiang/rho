use pretty_assertions::assert_eq;

use super::{format_updated_ago, session_picker};
use crate::session::SessionSummary;

// Covers: resume picker must not show a raw unix epoch as the recency signal
// Owner: tui session_picker pure formatting
#[test]
fn format_updated_ago_uses_relative_units() {
    let now: u64 = 1_700_000_000;
    let cases = [
        (now, "0s ago"),
        (now.saturating_sub(1), "1s ago"),
        (now.saturating_sub(59), "59s ago"),
        (now.saturating_sub(60), "1m ago"),
        (now.saturating_sub(59 * 60), "59m ago"),
        (now.saturating_sub(60 * 60), "1h ago"),
        (now.saturating_sub(47 * 60 * 60), "47h ago"),
        (now.saturating_sub(48 * 60 * 60), "2d ago"),
        (now.saturating_sub(10 * 24 * 60 * 60), "10d ago"),
        // Future timestamps (clock skew) clamp to the present.
        (now.saturating_add(120), "0s ago"),
    ];
    for (updated_at, expected) in cases {
        assert_eq!(
            format_updated_ago(updated_at, now),
            expected,
            "updated_at={updated_at} now={now}"
        );
    }
}

// Covers: /resume must not offer sessions with nothing to resume (a process
// killed before its first save, or an older Rho's unprompted ACP session),
// nor the session already open.
// Owner: tui session_picker
#[test]
fn session_picker_offers_only_other_sessions_with_messages() {
    let session = |id: &str, message_count| SessionSummary {
        id: id.into(),
        path: format!("/sessions/{id}.jsonl").into(),
        cwd: "/work".into(),
        created_at: 0,
        updated_at: 0,
        message_count,
        title: None,
        first_user_message: None,
        last_user_message: None,
    };
    let picker = session_picker(
        vec![session("empty", 0), session("open", 2), session("saved", 2)],
        Some("open"),
    );
    let offered: Vec<&str> = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect();
    assert_eq!(offered, vec!["saved"]);
}
