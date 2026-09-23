use pretty_assertions::assert_eq;

use super::{format_exit_receipt, ExitReceipt};

// Covers: receipt omits empty usage, strips multiline/control titles, and
// never prints the full session id
// Owner: tui exit receipt
#[test]
fn formats_compact_session_receipt() {
    let full_id = "abcdef12-3456-7890-abcd-ef1234567890";
    let cases = [
        (
            ExitReceipt {
                session_id: full_id.into(),
                title: Some("fix the flaky pty harness".into()),
                total_cost_usd_micros: Some(420_000),
                input_tokens: Some(128_400),
                output_tokens: Some(9_200),
                cache_hit_percent: Some(41.2),
            },
            /*has_usage*/ true,
        ),
        (
            ExitReceipt {
                session_id: full_id.into(),
                title: None,
                total_cost_usd_micros: None,
                input_tokens: None,
                output_tokens: None,
                cache_hit_percent: None,
            },
            /*has_usage*/ false,
        ),
        (
            ExitReceipt {
                session_id: full_id.into(),
                title: Some("  ".into()),
                total_cost_usd_micros: Some(42_000),
                input_tokens: Some(128),
                output_tokens: None,
                cache_hit_percent: None,
            },
            /*has_usage*/ true,
        ),
        (
            ExitReceipt {
                session_id: full_id.into(),
                title: Some("untitled work".into()),
                total_cost_usd_micros: None,
                input_tokens: None,
                output_tokens: Some(50),
                cache_hit_percent: Some(0.0),
            },
            /*has_usage*/ true,
        ),
        (
            ExitReceipt {
                session_id: full_id.into(),
                title: Some("line one\u{07}\nline two\x1b[31m".into()),
                total_cost_usd_micros: None,
                input_tokens: None,
                output_tokens: None,
                cache_hit_percent: None,
            },
            /*has_usage*/ false,
        ),
    ];

    for (receipt, has_usage) in cases {
        let rendered = format_exit_receipt(&receipt, /*styled*/ false);
        assert!(
            !rendered.contains(full_id),
            "receipt must not print the full session id"
        );
        assert!(
            rendered.chars().all(|ch| ch == '\n' || !ch.is_control()),
            "receipt must be stdout-safe: {rendered:?}"
        );
        let expected_lines = if has_usage { 3 } else { 2 };
        assert_eq!(rendered.lines().count(), expected_lines, "{rendered:?}");
    }
}
