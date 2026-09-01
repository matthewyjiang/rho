//! Text helpers shared by single-pane overlay panels.
//!
//! Panels decide what to say; these helpers only lay text out: a heading with
//! a right-aligned status, wrapped note lines under a fixed indent, and
//! one-line truncation.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{
    render::{display_width, truncate_one_line, wrap_line_at_whitespace},
    theme::Theme,
};

/// Bold `label` on the left and dim `status` right-aligned. The label yields
/// width to the status; an empty status renders the label alone.
pub(super) fn heading_with_status(label: &str, status: &str, width: usize) -> Line<'static> {
    let label_style = Theme::text().add_modifier(Modifier::BOLD);
    if status.is_empty() || width == 0 {
        return Line::from(Span::styled(truncate_to(label, width), label_style));
    }
    let gap = 2;
    let status_width = display_width(status);
    let label_budget = width.saturating_sub(status_width.saturating_add(gap));
    let label = truncate_to(label, label_budget);
    let pad = width
        .saturating_sub(display_width(&label))
        .saturating_sub(status_width);
    Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(status.to_string(), Theme::dim()),
    ])
}

/// Wrap `text` at whitespace to fit `width`, prefixing every line with
/// `indent` spaces. The indent shrinks before the text does at tiny widths.
pub(super) fn indented_wrapped_lines(
    text: &str,
    indent: usize,
    width: usize,
    style: Style,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from(Span::styled("", style))];
    }
    let indent_width = indent.min(width.saturating_sub(1));
    let text_width = width.saturating_sub(indent_width).max(1);
    let indent = " ".repeat(indent_width);
    wrap_line_at_whitespace(text, text_width)
        .into_iter()
        .map(|part| {
            Line::from(Span::styled(
                format!("{indent}{}", part.trim_start()),
                style,
            ))
        })
        .collect()
}

pub(super) fn truncate_to(text: &str, width: usize) -> String {
    truncate_one_line(text, width.max(1))
}

#[cfg(test)]
#[path = "panel_text_tests.rs"]
mod tests;
