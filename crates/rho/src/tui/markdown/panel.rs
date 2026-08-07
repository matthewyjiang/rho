//! Shared bordered-panel mechanics for rendered markdown art blocks.
//!
//! Features such as mermaid diagrams and display math decide what art or
//! fallback source a closed block shows; this module owns the shared shape
//! those decisions collapse into and the framing of art rows.

use ratatui::text::{Line, Span};

use super::super::{render::display_width, theme::Theme};

/// Complete closed art block ready for the Markdown renderer.
#[derive(Debug)]
pub(super) enum ClosedPanel {
    /// Rendered Unicode art with the original source kept for copying.
    Art {
        title: &'static str,
        lines: Vec<Line<'static>>,
        source: String,
    },
    /// Unrendered block shown as literal source under a fallback title.
    SourceFallback { title: &'static str, source: String },
}

pub(super) fn panel_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let canvas_width = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| display_width(span.content.as_ref()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or_default();
    lines
        .into_iter()
        .map(|line| panel_line(line, width, canvas_width))
        .collect()
}

fn panel_line(mut line: Line<'static>, width: usize, canvas_width: usize) -> Line<'static> {
    let style = Theme::markdown_code_block();
    if width <= 1 {
        return line;
    }
    if width <= 3 {
        line.spans.insert(0, Span::styled("│", style));
        return line;
    }

    let content_width = width - 4;
    let line_width = line
        .spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum::<usize>();
    let left_padding = content_width.saturating_sub(canvas_width) / 2;
    let right_padding = content_width
        .saturating_sub(left_padding)
        .saturating_sub(line_width);
    line.spans.insert(
        0,
        Span::styled(format!("│ {}", " ".repeat(left_padding)), style),
    );
    line.spans.push(Span::styled(
        format!("{} │", " ".repeat(right_padding)),
        style,
    ));
    line
}
