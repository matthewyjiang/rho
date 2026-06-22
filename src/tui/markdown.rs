use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{
    render::{wrap_line_at_whitespace_ranges, wrap_line_hard},
    theme::Theme,
};

pub(super) fn push_wrapped_markdown(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    in_code_block: &mut bool,
) {
    lines.extend(markdown_lines(text, width, in_code_block));
}

pub(super) fn markdown_lines(
    text: &str,
    width: usize,
    in_code_block: &mut bool,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for raw_line in text.lines() {
        let code_fence = raw_line.trim_start().starts_with("```");
        if code_fence {
            lines.push(code_block_border(
                width,
                if *in_code_block { '╰' } else { '╭' },
            ));
            *in_code_block = !*in_code_block;
            continue;
        }

        if *in_code_block {
            lines.extend(code_block_content_lines(raw_line, width));
            continue;
        }

        if is_markdown_divider(raw_line) {
            lines.push(markdown_divider(width));
            continue;
        }

        lines.extend(wrap_styled_segments(
            &markdown_inline_segments(raw_line),
            width,
        ));
    }

    if lines.is_empty() && text.is_empty() {
        lines.push(Line::from(Span::styled(String::new(), Theme::text())));
    }

    lines
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StyledSegment {
    text: String,
    style: Style,
}

impl StyledSegment {
    fn new(text: String, style: Style) -> Self {
        Self { text, style }
    }
}

fn is_markdown_divider(line: &str) -> bool {
    let trimmed = line.trim();
    let mut chars = trimmed.chars().filter(|ch| !ch.is_whitespace());
    let Some(marker) = chars.next() else {
        return false;
    };
    matches!(marker, '-' | '*' | '_')
        && trimmed.chars().filter(|ch| !ch.is_whitespace()).count() >= 3
        && chars.all(|ch| ch == marker)
}

fn markdown_divider(width: usize) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width.max(1)), Theme::dim()))
}

fn code_block_border(width: usize, corner: char) -> Line<'static> {
    let width = width.max(1);
    let style = Theme::markdown_code_block();
    if width == 1 {
        return Line::from(Span::styled(corner.to_string(), style));
    }

    let closing_corner = if corner == '╭' { '╮' } else { '╯' };
    Line::from(Span::styled(
        format!(
            "{corner}{}{closing_corner}",
            "─".repeat(width.saturating_sub(2))
        ),
        style,
    ))
}

fn code_block_content_lines(line: &str, width: usize) -> Vec<Line<'static>> {
    let style = Theme::markdown_code_block();
    if width <= 1 {
        return wrap_line_hard(line, 1)
            .into_iter()
            .map(|chunk| Line::from(Span::styled(chunk, style)))
            .collect();
    }
    if width <= 3 {
        return wrap_line_hard(line, width.saturating_sub(1).max(1))
            .into_iter()
            .map(|chunk| Line::from(Span::styled(format!("│{chunk}"), style)))
            .collect();
    }

    let content_width = width - 4;
    wrap_line_hard(line, content_width.max(1))
        .into_iter()
        .map(|chunk| {
            let chunk_width = chunk.chars().count();
            let padding = " ".repeat(content_width.saturating_sub(chunk_width));
            Line::from(Span::styled(format!("│ {chunk}{padding} │"), style))
        })
        .collect()
}

fn markdown_inline_segments(line: &str) -> Vec<StyledSegment> {
    let mut segments = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        if let Some((start, marker_len, end, style)) = next_markdown_span(rest) {
            if start > 0 {
                segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
            }
            let content_start = start + marker_len;
            let marked_end = end + marker_len;
            segments.push(StyledSegment::new(
                rest[content_start..end].to_string(),
                style,
            ));
            rest = &rest[marked_end..];
        } else {
            segments.push(StyledSegment::new(rest.to_string(), Theme::text()));
            break;
        }
    }
    segments
}

fn next_markdown_span(line: &str) -> Option<(usize, usize, usize, Style)> {
    let candidates = [
        next_delimited(line, "`", Theme::markdown_inline_code()),
        next_delimited(line, "**", Theme::markdown_bold()),
        next_delimited(line, "*", Theme::markdown_italic()),
        next_delimited(line, "_", Theme::markdown_italic()),
    ];
    candidates
        .into_iter()
        .flatten()
        .min_by_key(|(start, marker_len, _, _)| (*start, std::cmp::Reverse(*marker_len)))
}

fn next_delimited(line: &str, marker: &str, style: Style) -> Option<(usize, usize, usize, Style)> {
    let start = line.find(marker)?;
    if marker == "*" && line[start..].starts_with("**") {
        return None;
    }
    let content_start = start + marker.len();
    let relative_end = line[content_start..].find(marker)?;
    let end = content_start + relative_end;
    (end > content_start).then_some((start, marker.len(), end, style))
}

fn wrap_styled_segments(segments: &[StyledSegment], width: usize) -> Vec<Line<'static>> {
    let text = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let chars = segments
        .iter()
        .flat_map(|segment| segment.text.chars().map(|ch| (ch, segment.style)))
        .collect::<Vec<_>>();

    wrap_line_at_whitespace_ranges(&text, width)
        .into_iter()
        .map(|range| {
            let start = text[..range.start].chars().count();
            let end = start + text[range].chars().count();
            Line::from(merge_styled_chars(&chars[start..end]))
        })
        .collect()
}

fn merge_styled_chars(chars: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (ch, style) in chars {
        if let Some(last) = spans.last_mut() {
            if last.style == *style {
                last.content.to_mut().push(*ch);
                continue;
            }
        }
        spans.push(Span::styled(ch.to_string(), *style));
    }
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn line_styles(line: &Line<'_>) -> Vec<Style> {
        line.spans.iter().map(|span| span.style).collect()
    }

    #[test]
    fn styles_inline_code_bold_and_italic_without_markers() {
        let mut in_code_block = false;
        let lines = markdown_lines(
            "use `cargo test`, then **ship** the *fix*",
            80,
            &mut in_code_block,
        );

        assert_eq!(line_text(&lines[0]), "use cargo test, then ship the fix");
        let styles = line_styles(&lines[0]);
        assert!(styles.contains(&Theme::markdown_inline_code()));
        assert!(styles.contains(&Theme::markdown_bold()));
        assert!(styles.contains(&Theme::markdown_italic()));
        assert_eq!(Theme::markdown_bold().fg, None);
        assert_eq!(Theme::markdown_italic().fg, None);
    }

    #[test]
    fn renders_code_blocks_with_closed_borders() {
        let mut in_code_block = false;
        let lines = markdown_lines("```rust\nlet x = 1;\n```", 20, &mut in_code_block);

        assert_eq!(line_text(&lines[0]), "╭──────────────────╮");
        assert_eq!(line_text(&lines[1]), "│ let x = 1;       │");
        assert_eq!(line_text(&lines[2]), "╰──────────────────╯");
        assert_eq!(lines[1].spans[0].style, Theme::markdown_code_block());
    }

    #[test]
    fn renders_divider_lines() {
        let mut in_code_block = false;
        let lines = markdown_lines("before\n---\nafter", 20, &mut in_code_block);

        assert_eq!(line_text(&lines[0]), "before");
        assert_eq!(line_text(&lines[1]), "─".repeat(20));
        assert_eq!(lines[1].spans[0].style, Theme::dim());
        assert_eq!(line_text(&lines[2]), "after");
    }
}
