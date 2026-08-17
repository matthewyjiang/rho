use pretty_assertions::assert_eq;
use ratatui::{style::Style, text::Line};

use super::*;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn caption<'a>(candidates: &'a [&'a str], style: Style) -> DividerCaption<'a> {
    DividerCaption { candidates, style }
}

// Covers: right captions stay on the trailing rule, and scarce width drops them
// before a left caption so the two labels cannot collide.
// Owner: composer divider layout
#[test]
fn right_caption_stays_flush_and_yields_to_the_left_caption() {
    let rule = Style::new();
    let left_style = Style::new();
    let right_style = Style::new();
    let left = caption(&["shell"], left_style);
    let right = caption(&["advisor: m"], right_style);

    assert_eq!(
        line_text(&labeled_divider_line(None, Some(right), rule, 20)),
        "─────── advisor: m ─"
    );
    assert_eq!(
        line_text(&labeled_divider_line(Some(left), Some(right), rule, 24)),
        "─ shell ─── advisor: m ─"
    );
    assert_eq!(
        line_text(&labeled_divider_line(Some(left), Some(right), rule, 20)),
        "─ shell ────────────"
    );
    assert_eq!(
        line_text(&labeled_divider_line(Some(left), Some(right), rule, 4)),
        "────"
    );
}

// Covers: left-only captions keep the historical `─ label ────` shape.
// Owner: composer divider layout
#[test]
fn left_caption_keeps_prefix_and_trailing_fill() {
    let rule = Style::new();
    let left = caption(&["shell · in context", "shell"], rule);

    assert_eq!(
        line_text(&labeled_divider_line(Some(left), None, rule, 24)),
        "─ shell · in context ───"
    );
    assert_eq!(
        line_text(&labeled_divider_line(Some(left), None, rule, 10)),
        "─ shell ──"
    );
}
