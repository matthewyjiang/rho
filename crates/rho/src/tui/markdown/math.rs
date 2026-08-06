use std::panic::AssertUnwindSafe;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use txm::ratatui::Math;

use super::super::{render::display_width, theme::Theme};
use super::mermaid;

const MAX_SOURCE_BYTES: usize = 16 * 1024;
const MAX_SOURCE_LINES: usize = 256;
const MAX_RENDERED_LINES: usize = 128;
const MAX_RENDERED_WIDTH: usize = 240;
const MAX_INLINE_SOURCE_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MathFallback {
    Blank,
    SourceBytes,
    SourceLines,
    Parse,
    Panic,
    TooWide,
    OutputLines,
    EmptyOutput,
}

impl MathFallback {
    fn panel_title(self) -> &'static str {
        match self {
            Self::TooWide => "MATH · PANE TOO NARROW",
            Self::Blank
            | Self::SourceBytes
            | Self::SourceLines
            | Self::Parse
            | Self::Panic
            | Self::OutputLines
            | Self::EmptyOutput => "MATH · NOT RENDERED",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MathRender {
    Rendered(Vec<Line<'static>>),
    Fallback(MathFallback),
}

/// Complete closed display-math block ready for the Markdown renderer.
#[derive(Debug)]
pub(super) enum ClosedDisplayMath {
    Art {
        lines: Vec<Line<'static>>,
        source: String,
    },
    SourceFallback {
        title: &'static str,
        source: String,
    },
}

/// How far a closed or still-open display-math block extends from `lines[0]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DisplayMathSpan {
    /// Fully closed block spanning `line_count` source lines.
    Complete { line_count: usize },
    /// Opening `$$` with no closer yet; the rest of the buffer stays mutable.
    Incomplete,
}

pub(super) fn take_closed_display_math(lines: &[&str]) -> Option<(String, usize)> {
    match display_math_span(lines)? {
        DisplayMathSpan::Complete { line_count } => {
            Some((display_math_source(&lines[..line_count]), line_count))
        }
        DisplayMathSpan::Incomplete => None,
    }
}

pub(super) fn display_math_span(lines: &[&str]) -> Option<DisplayMathSpan> {
    let first = *lines.first()?;
    let trimmed = first.trim();
    if !trimmed.starts_with("$$") {
        return None;
    }

    let after_open = &trimmed[2..];
    if after_open.is_empty() {
        // Multi-line form: lone $$ opener.
        for (offset, line) in lines.iter().enumerate().skip(1) {
            if line.trim() == "$$" {
                return Some(DisplayMathSpan::Complete {
                    line_count: offset + 1,
                });
            }
        }
        return Some(DisplayMathSpan::Incomplete);
    }

    if let Some(close_rel) = after_open.find("$$") {
        let trailing = after_open[close_rel + 2..].trim();
        if trailing.is_empty() {
            return Some(DisplayMathSpan::Complete { line_count: 1 });
        }
    }

    // `$$partial` without a closer stays ordinary prose so streaming dollars do
    // not swallow the rest of the assistant message.
    None
}

fn display_math_source(lines: &[&str]) -> String {
    debug_assert!(!lines.is_empty());
    if lines.len() == 1 {
        let trimmed = lines[0].trim();
        debug_assert!(trimmed.starts_with("$$") && trimmed.ends_with("$$"));
        let inner = &trimmed[2..trimmed.len() - 2];
        return inner.trim().to_owned();
    }

    // Multi-line: drop the opening and closing $$ lines.
    lines[1..lines.len() - 1].join("\n")
}

pub(super) fn render_closed_display_math(source: String, inner_width: usize) -> ClosedDisplayMath {
    match render_math(&source, inner_width) {
        MathRender::Rendered(lines) => ClosedDisplayMath::Art { lines, source },
        MathRender::Fallback(reason) => ClosedDisplayMath::SourceFallback {
            title: reason.panel_title(),
            source,
        },
    }
}

pub(super) fn render_math(source: &str, inner_width: usize) -> MathRender {
    match std::panic::catch_unwind(AssertUnwindSafe(|| render_inner(source, inner_width))) {
        Ok(result) => result,
        Err(_) => MathRender::Fallback(MathFallback::Panic),
    }
}

fn render_inner(source: &str, inner_width: usize) -> MathRender {
    if source.trim().is_empty() {
        return MathRender::Fallback(MathFallback::Blank);
    }
    if source.len() > MAX_SOURCE_BYTES {
        return MathRender::Fallback(MathFallback::SourceBytes);
    }
    if source.lines().count() > MAX_SOURCE_LINES {
        return MathRender::Fallback(MathFallback::SourceLines);
    }
    if inner_width == 0 {
        return MathRender::Fallback(MathFallback::TooWide);
    }

    let style = Theme::markdown_code_block();
    let math = match Math::new(source) {
        Ok(math) => math.style(style),
        Err(_) => return MathRender::Fallback(MathFallback::Parse),
    };

    let size = math.size();
    if size.width == 0 || size.height == 0 {
        return MathRender::Fallback(MathFallback::EmptyOutput);
    }
    if size.height as usize > MAX_RENDERED_LINES {
        return MathRender::Fallback(MathFallback::OutputLines);
    }
    if size.width as usize > MAX_RENDERED_WIDTH || size.width as usize > inner_width {
        return MathRender::Fallback(MathFallback::TooWide);
    }

    let mut lines = Vec::with_capacity(size.height as usize);
    for text in visible_rows(&math) {
        let width = display_width(&text);
        if width > inner_width {
            return MathRender::Fallback(MathFallback::TooWide);
        }
        lines.push(Line::from(Span::styled(text, style)));
    }

    if lines.is_empty() {
        return MathRender::Fallback(MathFallback::EmptyOutput);
    }

    MathRender::Rendered(lines)
}

/// Non-blank rendered rows, top to bottom.
///
/// TXM often pads with blank rows (outer margins and nested-fraction gaps), so
/// whitespace-only rows are dropped to keep output tight around real glyphs.
fn visible_rows(math: &Math) -> Vec<String> {
    let size = math.size();
    let area = Rect::new(0, 0, size.width, size.height);
    let mut buffer = Buffer::empty(area);
    Widget::render(math, area, &mut buffer);

    let mut rows = Vec::with_capacity(size.height as usize);
    for y in 0..size.height {
        let mut text = String::new();
        let mut x = 0u16;
        while x < size.width {
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            if !symbol.is_empty() {
                text.push_str(symbol);
            }
            x = x.saturating_add(1);
        }
        if !text.chars().all(char::is_whitespace) {
            rows.push(text);
        }
    }
    rows
}

/// Single-row rendering of an inline `$...$` formula.
///
/// Returns `None` when the formula needs more than one terminal row (fractions,
/// stacked limits, mixed scripts), fails to parse, or exceeds inline limits; the
/// caller keeps the literal source text in that case.
pub(super) fn render_inline_math(source: &str) -> Option<String> {
    std::panic::catch_unwind(AssertUnwindSafe(|| render_inline_inner(source))).unwrap_or_default()
}

fn render_inline_inner(source: &str) -> Option<String> {
    if source.trim().is_empty() || source.len() > MAX_INLINE_SOURCE_BYTES || source.contains('\n') {
        return None;
    }

    let math = Math::new(source).ok()?;
    let size = math.size();
    if size.width == 0 || size.height == 0 || size.width as usize > MAX_RENDERED_WIDTH {
        return None;
    }

    let mut rows = visible_rows(&math);
    if rows.len() != 1 {
        return None;
    }
    let row = rows.pop().expect("rows has exactly one element");
    Some(row.trim().to_owned())
}

pub(super) fn panel_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    mermaid::panel_lines(lines, width)
}

#[cfg(test)]
#[path = "math_tests.rs"]
mod tests;
