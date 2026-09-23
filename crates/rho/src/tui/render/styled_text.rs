//! Width-bounded styled rows shared by overlays and picker panes.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{display_width, truncate_one_line, wrap_line_at_whitespace};

/// Wrap multi-line `text` to `width`, one styled row per wrapped part. Blank
/// source lines stay blank rows; empty text yields one blank row.
pub(in crate::tui) fn wrap_text_lines(
    text: &str,
    width: usize,
    style: Style,
) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::raw("")];
    }
    text.lines()
        .flat_map(|line| {
            if line.is_empty() {
                vec![Line::raw("")]
            } else {
                wrap_line_at_whitespace(line, width)
                    .into_iter()
                    .map(|part| Line::from(Span::styled(part.to_owned(), style)))
                    .collect()
            }
        })
        .collect()
}

/// Cut a styled row at `width` columns, ending a cut span in an ellipsis.
pub(in crate::tui) fn clip_line(line: Line<'static>, width: usize) -> Line<'static> {
    let mut used = 0usize;
    let mut spans = Vec::with_capacity(line.spans.len());
    for span in line.spans {
        if used >= width {
            break;
        }
        let span_width = display_width(&span.content);
        if used + span_width <= width {
            used += span_width;
            spans.push(span);
            continue;
        }
        spans.push(Span::styled(
            truncate_one_line(&span.content, width - used),
            span.style,
        ));
        break;
    }
    Line::from(spans)
}

/// [`clip_line`], then pad with spaces to exactly `width` columns so a
/// trailing gutter or border lines up.
pub(in crate::tui) fn fit_line(line: Line<'static>, width: usize) -> Line<'static> {
    let mut line = clip_line(line, width);
    let used = line
        .spans
        .iter()
        .map(|span| display_width(&span.content))
        .sum::<usize>();
    if used < width {
        line.spans.push(Span::raw(" ".repeat(width - used)));
    }
    line
}
