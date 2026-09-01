use pretty_assertions::assert_eq;

use super::*;

fn text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

// Covers: the status hugs the right edge and the label gives way first when
// the panel is narrow.
// Owner: pure layout
#[test]
fn heading_right_aligns_status_and_truncates_label() {
    let cases = [
        (
            "Providers",
            "⠙ checking",
            30,
            "Providers           ⠙ checking",
        ),
        ("Providers", "", 30, "Providers"),
        (
            "A very long section label",
            "failed",
            16,
            "A very …  failed",
        ),
    ];
    for (label, status, width, expected) in cases {
        assert_eq!(
            text(&heading_with_status(label, status, width)),
            expected,
            "label={label:?} status={status:?} width={width}"
        );
    }
}

// Covers: wrapped continuation lines keep the requested indent, and a width
// smaller than the indent still leaves room for text.
// Owner: pure layout
#[test]
fn wrapped_lines_keep_indent() {
    let style = Style::default();
    let lines = indented_wrapped_lines("run /login anthropic to add a key", 4, 20, style);
    assert_eq!(
        lines.iter().map(text).collect::<Vec<_>>(),
        vec!["    run /login ", "    anthropic to add", "    a key"]
    );

    let tiny = indented_wrapped_lines("abc", 4, 3, style);
    assert_eq!(
        tiny.iter().map(text).collect::<Vec<_>>(),
        vec!["  a", "  b", "  c"]
    );
}
