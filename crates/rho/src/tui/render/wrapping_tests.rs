use super::*;
use pretty_assertions::assert_eq;

// Covers: a wrap boundary must not detach combining marks, emoji modifiers,
// regional indicators, or keycap selectors, even in a one-column viewport.
// Owner: pure wrap math; PTY rendering cannot reveal the source byte partition.
#[test]
fn wrap_boundaries_preserve_whole_graphemes() {
    for (text, width, expected) in [
        ("e\u{301}x", 1, vec!["e\u{301}", "x"]),
        ("👩🏽‍💻x", 2, vec!["👩🏽‍💻", "x"]),
        ("🇺🇸🇨🇦", 2, vec!["🇺🇸", "🇨🇦"]),
        ("1️⃣x", 1, vec!["1️⃣", "x"]),
        (" \u{301}x", 1, vec![" \u{301}", "x"]),
    ] {
        for ranges in [
            wrap_line_at_whitespace_ranges(text, width),
            hard_wrap_ranges(text, width),
        ] {
            let rows: Vec<_> = ranges.into_iter().map(|range| &text[range]).collect();
            assert_eq!(rows, expected, "text {text:?}, width {width}");
        }
    }
}
