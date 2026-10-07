use pretty_assertions::assert_eq;
use ratatui::text::{Line, Span};

use super::{needle, scan_line, step_focus, Needle, ScanBuffers, SearchStep, TranscriptMatch};
use crate::tui::Theme;

fn line_matches(line: &Line<'_>, needle: &Needle) -> Vec<(usize, usize)> {
    let mut hits = Vec::new();
    scan_line(line, needle, &mut ScanBuffers::default(), |columns| {
        hits.push((columns.start, columns.end));
    });
    hits
}

// Covers: hits land on the painted display columns, case-insensitively,
// across span boundaries, wide glyphs, and multi-scalar graphemes, without
// matching code-block COPY chrome or overlapping a previous hit.
// Owner: transcript search matcher (pure).
#[test]
fn line_matches_report_display_columns() {
    let copy = || {
        Span::styled(
            " COPY ",
            Theme::markdown_code_copy_button(/*hovered*/ false),
        )
    };
    // Expected hits as (start, end) display columns.
    let cases = [
        ("empty query", Line::raw("anything"), "", vec![]),
        (
            "case-insensitive",
            Line::raw(" Error: error"),
            "ERROR",
            vec![(1, 6), (8, 13)],
        ),
        (
            "across spans",
            Line::from(vec![Span::raw(" pa"), Span::raw("th.rs")]),
            "path",
            vec![(1, 5)],
        ),
        (
            "wide glyphs shift columns",
            Line::raw("日本 path"),
            "path",
            vec![(5, 9)],
        ),
        (
            "hit covers a wide glyph",
            Line::raw("a日本"),
            "日",
            vec![(1, 3)],
        ),
        (
            "graphemes paint as one cell run",
            Line::raw("👩\u{200d}💻 path"),
            "path",
            vec![(3, 7)],
        ),
        (
            "combining mark highlights its grapheme",
            Line::raw("e\u{301}x"),
            "\u{301}",
            vec![(0, 1)],
        ),
        (
            "non-overlapping",
            Line::raw("aaaa"),
            "aa",
            vec![(0, 2), (2, 4)],
        ),
        (
            "copy button is not text",
            Line::from(vec![Span::raw("─"), copy(), Span::raw("─")]),
            "copy",
            vec![],
        ),
    ];
    for (case, line, query, expected) in cases {
        assert_eq!(line_matches(&line, &needle(query)), expected, "{case}");
    }
}

// Covers: a new query focuses the nearest hit above the starting viewport,
// Up/Down step older/newer with wraparound at both ends, and a focus left
// stale by a shrinking match list stays in range.
// Owner: transcript search navigation (pure).
#[test]
fn step_focus_searches_upward_and_wraps() {
    let hits: Vec<TranscriptMatch> = [3, 10, 20]
        .into_iter()
        .map(|line| TranscriptMatch {
            line,
            columns: 0..1,
        })
        .collect();
    let cases = [
        ("query above anchor", None, SearchStep::Query, 15, Some(1)),
        (
            "query ignores old focus",
            Some(0),
            SearchStep::Query,
            21,
            Some(2),
        ),
        (
            "query wraps when nothing above",
            None,
            SearchStep::Query,
            2,
            Some(2),
        ),
        ("older", Some(1), SearchStep::Older, 0, Some(0)),
        ("older wraps", Some(0), SearchStep::Older, 0, Some(2)),
        ("newer", Some(1), SearchStep::Newer, 0, Some(2)),
        ("newer wraps", Some(2), SearchStep::Newer, 0, Some(0)),
        ("stale focus clamps", Some(9), SearchStep::Older, 0, Some(1)),
    ];
    for (case, focus, step, anchor, expected) in cases {
        assert_eq!(step_focus(&hits, focus, step, anchor), expected, "{case}");
    }
    assert_eq!(step_focus(&[], None, SearchStep::Query, 5), None);
}
