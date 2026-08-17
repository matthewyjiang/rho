use pretty_assertions::assert_eq;
use ratatui::{style::Style, text::Line};

use super::*;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn caption(candidates: &[&str]) -> Option<DividerCaption> {
    DividerCaption::new(candidates.iter().copied(), Style::new())
}

// Covers: both-side fitting keeps the longest right caption, shrinking the
// left first, then degrades/drops the right before dropping the left.
// Owner: composer divider layout
#[test]
fn captions_keep_the_right_identity_and_shrink_the_left_first() {
    let rule = Style::new();
    // Same longest-first chain shell mode supplies. Copied here so layout
    // policy does not import the feature module.
    let shell: &[&str] = &["shell · included in context", "shell · in context", "shell"];
    let advisor: &[&str] = &["advisor: grok-4.5", "advisor"];
    let advisor_m: &[&str] = &["advisor: m"];
    let shell_only: &[&str] = &["shell"];

    let cases = [
        (
            "full both",
            Some(shell),
            Some(advisor),
            52,
            "─ shell · included in context ── advisor: grok-4.5 ─",
        ),
        // Discriminator vs longest-left-first: at 43 the long shell still
        // fits with "advisor", but the policy must keep the model name.
        (
            "shrink shell, keep model",
            Some(shell),
            Some(advisor),
            43,
            "─ shell · in context ── advisor: grok-4.5 ─",
        ),
        (
            "shortest shell, keep model",
            Some(shell),
            Some(advisor),
            30,
            "─ shell ── advisor: grok-4.5 ─",
        ),
        (
            "degrade advisor",
            Some(shell),
            Some(advisor),
            29,
            "─ shell ─────────── advisor ─",
        ),
        (
            "drop advisor",
            Some(shell),
            Some(advisor),
            19,
            "─ shell ───────────",
        ),
        ("bare rule", Some(shell), Some(advisor), 4, "────"),
        (
            "right only long",
            None,
            Some(advisor),
            22,
            "── advisor: grok-4.5 ─",
        ),
        (
            "right only flush",
            None,
            Some(advisor_m),
            20,
            "─────── advisor: m ─",
        ),
        (
            "both short labels",
            Some(shell_only),
            Some(advisor_m),
            24,
            "─ shell ─── advisor: m ─",
        ),
        (
            "right yields when pair is too wide",
            Some(shell_only),
            Some(advisor_m),
            20,
            "─ shell ────────────",
        ),
        (
            "left fallback without right",
            Some(shell),
            None,
            24,
            "─ shell · in context ───",
        ),
        ("shortest left only", Some(shell), None, 10, "─ shell ──"),
    ];

    for (name, left, right, width, expected) in cases {
        let line =
            labeled_divider_line(left.and_then(caption), right.and_then(caption), rule, width);
        assert_eq!(line_text(&line), expected, "{name} width={width}");
        assert_eq!(
            display_width(&line_text(&line)),
            width,
            "{name} painted width {width}"
        );
    }
}
