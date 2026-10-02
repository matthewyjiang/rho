//! Code body paint: one source stream highlighted by its fence language.

use ratatui::text::{Line, Span};

use super::{
    render::{
        display_width, pad_spaces, slice_spans_by_bytes, soft_wrap_visible_ranges,
        wrap_line_at_whitespace_ranges,
    },
    syntax::{
        spans_from_segments_with_matches, BlockHighlighter, HighlightSegment,
        MAX_TOOL_SYNTAX_LINES, MAX_TOOL_SYNTAX_LINE_BYTES,
    },
    theme::Theme,
};

/// Content column indent under the tool tree (matches tool_card_render).
const CHILD_CONTENT_INDENT: &str = "    ";

/// Stateful highlighter for a code-syntax body. Feed lines in order so
/// multi-line tokens (strings, comments) keep their state.
pub(super) struct CodeSyntax {
    language: String,
    highlighter: Option<BlockHighlighter>,
    highlighted_lines: usize,
}

impl CodeSyntax {
    pub(super) fn new(language: &str) -> Self {
        Self {
            language: language.to_string(),
            highlighter: BlockHighlighter::for_language(language),
            highlighted_lines: 0,
        }
    }

    /// Paint one logical source line, wrapped under the tree. Returns rows.
    pub(super) fn paint_line(
        &mut self,
        line: &str,
        width: usize,
        out: &mut Vec<Line<'static>>,
    ) -> usize {
        let segments = self.highlight(line);
        let spans = spans_from_segments_with_matches(&segments, Theme::text(), &[]);
        let content_width = width
            .saturating_sub(display_width(CHILD_CONTENT_INDENT))
            .max(1);
        // Word wrap like plain bodies, so their row estimate holds for code;
        // only unbroken runs split at the width.
        let ranges: Vec<_> =
            soft_wrap_visible_ranges(line, wrap_line_at_whitespace_ranges(line, content_width))
                .collect();
        if ranges.is_empty() {
            out.push(body_row(Vec::new(), width));
            return 1;
        }
        let rows = ranges.len();
        out.extend(
            ranges
                .into_iter()
                .map(|range| body_row(slice_spans_by_bytes(&spans, range.start, range.end), width)),
        );
        rows
    }

    /// Same budgets as grep and diff bodies: overlong lines stay plain and
    /// restart the stream so the next line does not inherit a desynced stack.
    fn highlight(&mut self, line: &str) -> Vec<HighlightSegment> {
        let plain = || {
            vec![HighlightSegment {
                text: line.to_string(),
                role: None,
            }]
        };
        if line.len() > MAX_TOOL_SYNTAX_LINE_BYTES {
            self.highlighter = BlockHighlighter::for_language(&self.language);
            return plain();
        }
        if self.highlighted_lines >= MAX_TOOL_SYNTAX_LINES {
            return plain();
        }
        match self.highlighter.as_mut() {
            Some(highlighter) => {
                self.highlighted_lines += 1;
                highlighter.highlight_line(line)
            }
            None => plain(),
        }
    }
}

fn body_row(content: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(CHILD_CONTENT_INDENT, Theme::tool_tree())];
    spans.extend(content);
    let used: usize = spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum();
    if used < width {
        spans.push(Span::styled(pad_spaces(width - used), Theme::text()));
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "tool_code_tests.rs"]
mod tests;
