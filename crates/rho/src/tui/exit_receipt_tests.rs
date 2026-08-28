use pretty_assertions::assert_eq;

use super::{format_exit_receipt, ExitReceipt};

// Covers: receipt omits empty usage, falls back from a blank title, sanitizes
// multiline/control titles, and never prints the full session id
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
            concat!(
                "session saved: fix the flaky pty harness\n",
                "  resume  rho --resume abcdef12\n",
                "  usage   $0.420 · 128.4K in / 9.2K out · 41% cache hit"
            ),
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
            concat!(
                "session saved: abcdef12\n",
                "  resume  rho --resume abcdef12"
            ),
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
            concat!(
                "session saved: abcdef12\n",
                "  resume  rho --resume abcdef12\n",
                "  usage   $0.042 · 128 in"
            ),
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
            concat!(
                "session saved: untitled work\n",
                "  resume  rho --resume abcdef12\n",
                "  usage   50 out · 0% cache hit"
            ),
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
            concat!(
                "session saved: line one\n",
                "  resume  rho --resume abcdef12"
            ),
        ),
    ];

    for (receipt, expected) in cases {
        let rendered = format_exit_receipt(&receipt, /*styled*/ false);
        assert!(
            !rendered.contains(full_id),
            "receipt must not print the full session id"
        );
        assert_eq!(rendered, expected);
    }
}
