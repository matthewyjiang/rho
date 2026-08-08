//! Grep/search body paint: path headers, language-aware match lines, pattern overlay.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{
    render::{display_width, hard_wrap_styled_spans, wrap_line_hard},
    syntax::{
        match_byte_ranges, spans_from_segments_with_matches, spans_plain_with_matches,
        BlockHighlighter, HighlightSegment, MatchQuery, MAX_TOOL_SYNTAX_LINES,
    },
    theme::Theme,
};

/// Content column indent under the tool tree (matches tool_card_render).
const CHILD_CONTENT_INDENT: &str = "    ";

/// Stateful highlighter for grep `content` bodies:
/// ```text
/// path/or/[path#TAG]
/// 12 | source line
/// ... +3 more in this file
/// ```
pub(super) struct SearchSyntax {
    path: Option<String>,
    highlighter: Option<BlockHighlighter>,
    query: MatchQuery,
    highlighted_lines: usize,
}

impl SearchSyntax {
    pub(super) fn new(query: MatchQuery) -> Self {
        Self {
            path: None,
            highlighter: None,
            query,
            highlighted_lines: 0,
        }
    }

    /// Classify and style one logical body line. Returns terminal rows.
    pub(super) fn paint_line(
        &mut self,
        line: &str,
        width: usize,
        out: &mut Vec<Line<'static>>,
    ) -> usize {
        match classify_search_line(line) {
            SearchLine::Path { path, display } => {
                self.set_path(path);
                push_body_spans(
                    out,
                    display,
                    width,
                    vec![Span::styled(display.to_string(), Theme::tool_path())],
                )
            }
            SearchLine::Content { prefix, source } => {
                let plain = Theme::text();
                let match_ranges = match_byte_ranges(source, &self.query);
                let source_spans = self.paint_source(source, plain, &match_ranges);
                // Prefix (`N | `) stays dim meta; source carries syntax + match.
                let mut spans = vec![Span::styled(prefix.to_string(), Theme::tool_meta())];
                spans.extend(source_spans);
                let full_text = format!("{prefix}{source}");
                push_body_spans(out, &full_text, width, spans)
            }
            SearchLine::Meta => push_body_spans(
                out,
                line,
                width,
                vec![Span::styled(line.to_string(), Theme::tool_meta())],
            ),
            SearchLine::Plain => {
                let match_ranges = match_byte_ranges(line, &self.query);
                let spans = spans_plain_with_matches(line, Theme::text(), &match_ranges);
                push_body_spans(out, line, width, spans)
            }
        }
    }

    /// Terminal row count for one line without language paint (toggle / hidden).
    pub(super) fn estimate_rows(line: &str, width: usize) -> usize {
        let prefix = CHILD_CONTENT_INDENT;
        let content_width = width.saturating_sub(display_width(prefix)).max(1);
        wrap_line_hard(line, content_width).len().max(1)
    }

    fn paint_source(
        &mut self,
        source: &str,
        plain: Style,
        match_ranges: &[(usize, usize)],
    ) -> Vec<Span<'static>> {
        let segments = self.highlight_source(source);
        spans_from_segments_with_matches(&segments, plain, match_ranges)
    }

    fn highlight_source(&mut self, source: &str) -> Vec<HighlightSegment> {
        if self.highlighted_lines >= MAX_TOOL_SYNTAX_LINES {
            return vec![HighlightSegment {
                text: source.to_string(),
                role: None,
            }];
        }
        match self.highlighter.as_mut() {
            Some(hl) => {
                self.highlighted_lines += 1;
                hl.highlight_line(source)
            }
            None => vec![HighlightSegment {
                text: source.to_string(),
                role: None,
            }],
        }
    }

    fn set_path(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        if self.path.as_deref() == Some(path) {
            return;
        }
        self.path = Some(path.to_string());
        self.highlighter = BlockHighlighter::for_path(path);
        self.highlighted_lines = 0;
    }
}

#[derive(Debug)]
enum SearchLine<'a> {
    /// File section header: plain path or `[path#TAG]`.
    Path { path: &'a str, display: &'a str },
    /// Match row: `N | source`.
    Content { prefix: &'a str, source: &'a str },
    /// Truncation / summary chrome.
    Meta,
    /// Fallback plain text.
    Plain,
}

fn classify_search_line(line: &str) -> SearchLine<'_> {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return SearchLine::Plain;
    }
    if trimmed.starts_with("... ") {
        return SearchLine::Meta;
    }
    // Trailing stats: "5 matches in 2 files" / "no matches for ..."
    if looks_like_search_summary(trimmed) {
        return SearchLine::Meta;
    }
    if let Some((num, rest)) = trimmed.split_once(" | ") {
        if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            let prefix_len = num.len() + 3; // " | "
            let prefix = &trimmed[..prefix_len];
            let source = rest;
            return SearchLine::Content { prefix, source };
        }
    }
    if let Some(path) = path_from_hashline_header(trimmed) {
        return SearchLine::Path {
            path,
            display: trimmed,
        };
    }
    // Plain path header (content mode without hashline tags).
    if looks_like_path_header(trimmed) {
        return SearchLine::Path {
            path: trimmed,
            display: trimmed,
        };
    }
    SearchLine::Plain
}

fn path_from_hashline_header(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let (path, tag) = inner.rsplit_once('#')?;
    if path.is_empty() || tag.is_empty() {
        return None;
    }
    if !tag.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(path)
}

fn looks_like_path_header(line: &str) -> bool {
    // Content-mode path lines are single tokens without leading spaces and
    // usually contain a slash or a known extension. Avoid eating prose.
    if line.starts_with(' ') || line.contains(" | ") {
        return false;
    }
    if line.contains('/') || line.contains('\\') {
        return true;
    }
    // Bare file name with extension.
    Path::new_has_extension(line)
}

fn looks_like_search_summary(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains(" match")
        || lower.starts_with("no matches")
        || lower.contains("narrow")
        || (lower.contains("files")
            && (lower.contains("match") || lower.contains("shown") || lower.contains("total")))
}

/// Tiny helper so we do not pull std::path only for extension checks on headers.
struct Path;
impl Path {
    fn new_has_extension(name: &str) -> bool {
        let Some((_, ext)) = name.rsplit_once('.') else {
            return false;
        };
        !ext.is_empty()
            && ext.len() <= 10
            && ext
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-')
    }
}

fn push_body_spans(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    content_spans: Vec<Span<'static>>,
) -> usize {
    let prefix = CHILD_CONTENT_INDENT;
    let prefix_width = display_width(prefix);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let plain = content_spans
        .first()
        .map(|span| span.style)
        .unwrap_or_else(Theme::text);
    let wrapped = hard_wrap_styled_spans(text, &content_spans, content_width, plain);
    let count = wrapped.len().max(1);
    if wrapped.is_empty() {
        lines.push(pad_line(
            vec![
                Span::styled(prefix.to_string(), Theme::tool_tree()),
                Span::styled(String::new(), plain),
            ],
            width,
        ));
        return 1;
    }
    for chunk in wrapped {
        let mut spans = vec![Span::styled(prefix.to_string(), Theme::tool_tree())];
        spans.extend(chunk);
        lines.push(pad_line(spans, width));
    }
    count
}

fn pad_line(mut spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used = spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum::<usize>();
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), Theme::text()));
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "tool_search_tests.rs"]
mod tests;
