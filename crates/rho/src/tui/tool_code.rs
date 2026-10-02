//! Code body paint: one source stream highlighted by its fence language.

use ratatui::text::{Line, Span};

use super::{
    render::{
        display_width, pad_spaces, slice_spans_by_bytes, soft_wrap_visible_ranges,
        wrap_line_at_whitespace, wrap_line_at_whitespace_ranges,
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

    /// Python tool-call batches parse linearly in the measured workload: about
    /// 2.5 ms at 1 KiB, 10 ms at 4 KiB, and 40 ms at 16 KiB (optimized syntect,
    /// repeated read_file calls). Keep the Markdown-derived guard for other
    /// grammars; 4 KiB leaves normal batches room without a large frame stall.
    fn line_byte_limit(language: &str) -> usize {
        match language {
            "python" | "py" => 4 * 1024,
            _ => MAX_TOOL_SYNTAX_LINE_BYTES,
        }
    }

    fn line_warning(language: &str, line: &str) -> Option<String> {
        let limit = Self::line_byte_limit(language);
        (line.len() > limit).then(|| {
            format!(
                "syntax highlighting skipped: line has {} bytes; {language} line budget is {limit} bytes",
                line.len()
            )
        })
    }

    pub(super) fn estimate_rows(language: &str, line: &str, width: usize) -> usize {
        let estimate = |text: &str| super::tool_card_render::estimate_plain_body_rows(text, width);
        estimate(line) + Self::line_warning(language, line).map_or(0, |warning| estimate(&warning))
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
        let start = out.len();
        if let Some(warning) = Self::line_warning(&self.language, line) {
            for text in wrap_line_at_whitespace(&warning, content_width) {
                out.push(body_row(
                    vec![Span::styled(text.to_owned(), Theme::warning())],
                    width,
                ));
            }
        }
        // Word wrap like plain bodies, so their row estimate holds for code;
        // only unbroken runs split at the width.
        let ranges: Vec<_> =
            soft_wrap_visible_ranges(line, wrap_line_at_whitespace_ranges(line, content_width))
                .collect();
        if ranges.is_empty() {
            out.push(body_row(Vec::new(), width));
            return out.len() - start;
        }
        out.extend(
            ranges
                .into_iter()
                .map(|range| body_row(slice_spans_by_bytes(&spans, range.start, range.end), width)),
        );
        out.len() - start
    }

    /// Feed hidden source without wrapping or allocating terminal rows.
    pub(super) fn advance_line(&mut self, line: &str) {
        if line.len() > Self::line_byte_limit(&self.language) {
            self.highlighter = BlockHighlighter::for_language(&self.language);
        } else if self.highlighted_lines < MAX_TOOL_SYNTAX_LINES {
            if let Some(highlighter) = self.highlighter.as_mut() {
                self.highlighted_lines += 1;
                highlighter.advance_line(line);
            }
        }
    }

    /// Overlong lines stay plain with a visible budget notice and
    /// restart the stream so the next line does not inherit a desynced stack.
    fn highlight(&mut self, line: &str) -> Vec<HighlightSegment> {
        let plain = || {
            vec![HighlightSegment {
                text: line.to_string(),
                role: None,
            }]
        };
        if line.len() > Self::line_byte_limit(&self.language) {
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
