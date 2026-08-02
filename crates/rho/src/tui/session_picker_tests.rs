use pretty_assertions::assert_eq;

use super::format_updated_ago;

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
