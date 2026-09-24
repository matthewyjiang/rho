use pretty_assertions::assert_eq;

use super::{wrap_footer_parts, FOOTER_SEPARATOR};

// Covers: footer binds must wrap as whole segments instead of clipping mid-hint.
// Owner: tui footer layout
#[test]
fn wrap_footer_parts_keeps_segments_whole() {
    let parts = ["aaaa", "bbbb", "cccc", "dddd"];
    let joined = |segments: &[&str]| segments.join(FOOTER_SEPARATOR);
    // Width that fits exactly two segments plus one separator.
    let two_wide = 8 + FOOTER_SEPARATOR.chars().count();
    for (case, parts, width, expected) in [
        (
            "packs segments up to the width",
            &parts[..],
            two_wide,
            vec![joined(&["aaaa", "bbbb"]), joined(&["cccc", "dddd"])],
        ),
        (
            "one segment per line when narrow",
            &parts[..],
            5,
            parts.map(String::from).to_vec(),
        ),
        (
            "skips empty segments",
            &["", "aaaa", "", "bbbb"][..],
            80,
            vec![joined(&["aaaa", "bbbb"])],
        ),
        (
            "never splits an overlong segment",
            &["aaaaaaaaaaaa"][..],
            4,
            vec!["aaaaaaaaaaaa".to_string()],
        ),
    ] {
        assert_eq!(
            wrap_footer_parts(parts.iter().copied(), width),
            expected,
            "{case}"
        );
    }
}
