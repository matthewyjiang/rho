use ratatui::{
    style::Style,
    text::{Line, Span},
};

mod code_fence;
mod heading;
mod mermaid;
mod stream;
mod table;

pub(in crate::tui) use code_fence::{
    is_closing_fence, parse_opening_fence, update_code_block_state, CodeFenceState,
};
use code_fence::{mermaid_opening_fence, CodeFence};

use super::markdown_image::standalone_markdown_image;

pub(in crate::tui) use heading::HeadingLevel;
use heading::{heading_stream_state, parse_atx_heading, HeadingStreamState};
pub(super) use stream::{markdown_preview_width, markdown_stream_prefix};

#[cfg(test)]
#[path = "markdown/table_tests.rs"]
mod table_tests;

use super::{
    render::{char_display_width, display_width, wrap_line_at_whitespace_ranges, wrap_line_hard},
    theme::Theme,
};

pub(super) fn push_wrapped_markdown_without_copy_button_from_fence_state(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    state: &mut CodeFenceState,
) {
    lines.extend(
        render_markdown_from_fence_state(text, width, state, CodeBlockCopyButton::Hidden).lines,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodeBlockCopyButton {
    Visible,
    Hidden,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarkdownCodeBlock {
    pub(super) top_line: usize,
    pub(super) copy_columns: std::ops::Range<usize>,
    pub(super) text: String,
}

pub(super) struct RenderedMarkdown {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) code_blocks: Vec<MarkdownCodeBlock>,
    /// Standalone `![alt](path)` references, in source order.
    pub(super) image_sources: Vec<super::markdown_image::MarkdownImageSource>,
    /// Rendered fallback rows corresponding to `image_sources`.
    pub(super) image_rows: Vec<usize>,
}

pub(super) fn markdown_lines(
    text: &str,
    width: usize,
    in_code_block: &mut bool,
) -> Vec<Line<'static>> {
    render_markdown(text, width, in_code_block).lines
}

pub(super) fn render_markdown(
    text: &str,
    width: usize,
    in_code_block: &mut bool,
) -> RenderedMarkdown {
    render_markdown_with_copy_button(text, width, in_code_block, CodeBlockCopyButton::Visible)
}

/// Returns the start of the trailing block that can still change as markdown is appended.
///
/// Markdown is line-oriented except for fenced code blocks and tables. Keeping
/// the final block mutable lets the history cache promote completed blocks and
/// re-render only this suffix as streaming text arrives.
pub(super) fn incremental_markdown_tail_start(text: &str) -> usize {
    let mut lines = Vec::new();
    let mut offset = 0;
    for source_line in text.split_inclusive('\n') {
        let raw_line = source_line.strip_suffix('\n').unwrap_or(source_line);
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        lines.push((offset, raw_line));
        offset += source_line.len();
    }
    if lines.is_empty() {
        return 0;
    }

    let raw_lines = lines.iter().map(|(_, line)| *line).collect::<Vec<_>>();
    let mut line_index = 0;
    let mut trailing_block_start = 0;
    while line_index < raw_lines.len() {
        trailing_block_start = lines[line_index].0;
        if let Some(opening) = parse_opening_fence(raw_lines[line_index]) {
            line_index += 1;
            while line_index < raw_lines.len() {
                let closes_block = is_closing_fence(raw_lines[line_index], opening);
                line_index += 1;
                if closes_block {
                    break;
                }
            }
            continue;
        }
        if let Some(consumed_lines) = table::markdown_table_line_count(&raw_lines[line_index..]) {
            line_index += consumed_lines;
            continue;
        }
        line_index += 1;
    }
    trailing_block_start
}

fn render_markdown_with_copy_button(
    text: &str,
    width: usize,
    in_code_block: &mut bool,
    copy_button: CodeBlockCopyButton,
) -> RenderedMarkdown {
    let mut state = CodeFenceState::from_open_flag(*in_code_block);
    let rendered = render_markdown_from_fence_state(text, width, &mut state, copy_button);
    *in_code_block = state.is_open();
    rendered
}

fn render_markdown_from_fence_state(
    text: &str,
    width: usize,
    state: &mut CodeFenceState,
    copy_button: CodeBlockCopyButton,
) -> RenderedMarkdown {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut code_blocks = Vec::new();
    let mut image_sources = Vec::new();
    let mut image_rows = Vec::new();
    let mut active_code_block: Option<(usize, std::ops::Range<usize>, Vec<&str>)> = None;

    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut line_index = 0;
    let mut active_fence = state.active;
    while line_index < raw_lines.len() {
        let raw_line = raw_lines[line_index];
        if active_fence.is_none() {
            if let Some(opening) = mermaid_opening_fence(raw_line) {
                if let Some(closing_offset) = raw_lines[line_index + 1..]
                    .iter()
                    .position(|line| is_closing_fence(line, opening.fence))
                {
                    let closing_index = line_index + 1 + closing_offset;
                    let source = raw_lines[line_index + 1..closing_index].join("\n");
                    let inner_width = width.saturating_sub(4);
                    if let mermaid::MermaidRender::Rendered(diagram_lines) =
                        mermaid::render_mermaid(&source, inner_width)
                    {
                        let top_line = lines.len();
                        lines.push(code_block_border(width, '╭', copy_button, Some("MERMAID")));
                        lines.extend(mermaid::panel_lines(diagram_lines, width));
                        lines.push(code_block_border(width, '╰', copy_button, None));
                        if copy_button == CodeBlockCopyButton::Visible {
                            if let Some(copy_columns) = code_block_copy_columns(width) {
                                code_blocks.push(MarkdownCodeBlock {
                                    top_line,
                                    copy_columns,
                                    text: source,
                                });
                            }
                        }
                        line_index = closing_index + 1;
                        continue;
                    }
                }
            }
        }
        let opening_fence = (active_fence.is_none())
            .then(|| parse_opening_fence(raw_line))
            .flatten();
        let closing_fence = active_fence.is_some_and(|fence| is_closing_fence(raw_line, fence));
        if opening_fence.is_some() || closing_fence {
            if closing_fence {
                lines.push(code_block_border(width, '╰', copy_button, None));
                if let Some((top_line, copy_columns, content)) = active_code_block.take() {
                    code_blocks.push(MarkdownCodeBlock {
                        top_line,
                        copy_columns,
                        text: content.join("\n"),
                    });
                }
                active_fence = None;
            } else {
                active_fence = opening_fence;
                let top_line = lines.len();
                lines.push(code_block_border(width, '╭', copy_button, None));
                if copy_button == CodeBlockCopyButton::Visible {
                    if let Some(copy_columns) = code_block_copy_columns(width) {
                        active_code_block = Some((top_line, copy_columns, Vec::new()));
                    }
                }
            }
            state.active = active_fence;
            line_index += 1;
            continue;
        }

        if active_fence.is_some() {
            if let Some((_, _, content)) = &mut active_code_block {
                content.push(raw_line);
            }
            lines.extend(code_block_content_lines(raw_line, width));
            line_index += 1;
            continue;
        }

        if let Some((table_lines, consumed_lines)) =
            table::markdown_table_lines(&raw_lines[line_index..], width)
        {
            lines.extend(table_lines);
            line_index += consumed_lines;
            continue;
        }

        if let Some(heading) = parse_atx_heading(raw_line) {
            lines.extend(markdown_heading_lines(heading, width));
            line_index += 1;
            continue;
        }

        if is_markdown_divider(raw_line) {
            lines.push(markdown_divider(width));
            line_index += 1;
            continue;
        }

        if let Some(image) = standalone_markdown_image(raw_line) {
            image_rows.push(lines.len());
            let fallback = if image.alt.is_empty() {
                format!("[image: {}]", image.path)
            } else {
                format!("[image: {}]", image.alt)
            };
            lines.push(Line::styled(fallback, Theme::markdown_link()));
            image_sources.push(image);
            line_index += 1;
            continue;
        }

        lines.extend(wrap_styled_segments(
            &markdown_inline_segments(raw_line),
            width,
        ));
        line_index += 1;
    }

    if let Some((top_line, copy_columns, content)) = active_code_block {
        code_blocks.push(MarkdownCodeBlock {
            top_line,
            copy_columns,
            text: content.join("\n"),
        });
    }

    if lines.is_empty() && text.is_empty() {
        lines.push(Line::from(Span::styled(String::new(), Theme::text())));
    }

    state.active = active_fence;
    RenderedMarkdown {
        lines,
        code_blocks,
        image_sources,
        image_rows,
    }
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

fn code_block_border(
    width: usize,
    corner: char,
    copy_button: CodeBlockCopyButton,
    title: Option<&str>,
) -> Line<'static> {
    let width = width.max(1);
    let style = Theme::markdown_code_block();
    if width == 1 {
        return Line::from(Span::styled(corner.to_string(), style));
    }

    let closing_corner = if corner == '╭' { '╮' } else { '╯' };
    let copy_columns = (corner == '╭' && copy_button == CodeBlockCopyButton::Visible)
        .then(|| code_block_copy_columns(width))
        .flatten();
    let label = copy_columns
        .as_ref()
        .and_then(|_| code_block_copy_label(width));
    let prefix_width = copy_columns
        .as_ref()
        .map_or(width.saturating_sub(2), |columns| {
            columns.start.saturating_sub(1)
        });
    let title = title
        .map(|title| format!("─ {title} "))
        .filter(|title| display_width(title) <= prefix_width)
        .unwrap_or_default();
    let title_width = display_width(&title);
    let mut spans = vec![Span::styled(
        format!(
            "{corner}{title}{}",
            "─".repeat(prefix_width.saturating_sub(title_width))
        ),
        style,
    )];
    if let Some(label) = label {
        spans.push(Span::styled(
            label,
            Theme::markdown_code_copy_button(/*hovered*/ false),
        ));
    }
    spans.push(Span::styled(closing_corner.to_string(), style));
    Line::from(spans)
}

fn code_block_copy_label(width: usize) -> Option<&'static str> {
    if width >= 9 {
        Some(" COPY ")
    } else if width >= 6 {
        Some("COPY")
    } else {
        None
    }
}

fn code_block_copy_columns(width: usize) -> Option<std::ops::Range<usize>> {
    let label_width = display_width(code_block_copy_label(width)?);
    let start = width.saturating_sub(label_width + 1);
    Some(start..start + label_width)
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
            let chunk_width = display_width(&chunk);
            let padding = " ".repeat(content_width.saturating_sub(chunk_width));
            Line::from(Span::styled(format!("│ {chunk}{padding} │"), style))
        })
        .collect()
}

fn markdown_heading_lines(heading: heading::AtxHeading<'_>, width: usize) -> Vec<Line<'static>> {
    let heading_style = Theme::markdown_heading(heading.level);
    if heading.content.is_empty() {
        return vec![Line::from(Span::styled(String::new(), heading_style))];
    }

    let segments = markdown_inline_segments(heading.content)
        .into_iter()
        .map(|segment| StyledSegment::new(segment.text, heading_style.patch(segment.style)))
        .collect::<Vec<_>>();
    wrap_styled_segments(&segments, width)
}

fn markdown_inline_segments(line: &str) -> Vec<StyledSegment> {
    let mut segments = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        match next_markdown_span(rest) {
            Some(MarkdownSpan::Styled {
                start,
                marker_len,
                end,
                style,
            }) => {
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
            }
            Some(MarkdownSpan::Image { start, end, alt }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                // Inline images cannot reserve rows inside wrapped prose, so
                // they fall back to their alt text.
                if !alt.is_empty() {
                    segments.push(StyledSegment::new(alt, Theme::text()));
                }
                rest = &rest[end..];
            }
            Some(MarkdownSpan::Link {
                start,
                end,
                label,
                target,
            }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                segments.push(StyledSegment::new(label, Theme::text()));
                segments.push(StyledSegment::new(": ".to_string(), Theme::text()));
                segments.push(StyledSegment::new(target, Theme::markdown_link()));
                rest = &rest[end..];
            }
            Some(MarkdownSpan::RawUrl { start, end }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                segments.push(StyledSegment::new(
                    rest[start..end].to_string(),
                    Theme::markdown_link(),
                ));
                rest = &rest[end..];
            }
            None => {
                segments.push(StyledSegment::new(rest.to_string(), Theme::text()));
                break;
            }
        }
    }
    segments
}

#[derive(Debug)]
enum MarkdownSpan {
    Styled {
        start: usize,
        marker_len: usize,
        end: usize,
        style: Style,
    },
    Image {
        start: usize,
        end: usize,
        alt: String,
    },
    Link {
        start: usize,
        end: usize,
        label: String,
        target: String,
    },
    RawUrl {
        start: usize,
        end: usize,
    },
}

fn next_markdown_span(line: &str) -> Option<MarkdownSpan> {
    let candidates = [
        next_markdown_image_span(line),
        next_markdown_link(line),
        next_raw_url(line),
        next_delimited(line, "`", Theme::markdown_inline_code()),
        next_delimited(line, "**", Theme::markdown_bold()),
        next_delimited(line, "*", Theme::markdown_italic()),
        next_delimited(line, "_", Theme::markdown_italic()),
    ];
    candidates
        .into_iter()
        .flatten()
        .min_by_key(|span| match span {
            MarkdownSpan::Styled {
                start, marker_len, ..
            } => (*start, std::cmp::Reverse(*marker_len)),
            MarkdownSpan::Image { start, .. } => (*start, std::cmp::Reverse(1)),
            MarkdownSpan::Link { start, .. } => (*start, std::cmp::Reverse(1)),
            MarkdownSpan::RawUrl { start, .. } => (*start, std::cmp::Reverse(1)),
        })
}

fn next_markdown_image_span(line: &str) -> Option<MarkdownSpan> {
    let (image, range) = super::markdown_image::next_markdown_image(line)?;
    Some(MarkdownSpan::Image {
        start: range.start,
        end: range.end,
        alt: image.alt,
    })
}

fn next_markdown_link(line: &str) -> Option<MarkdownSpan> {
    let start = line.find('[')?;
    let after_label = start + 1;
    let close_label = line[after_label..].find(']')? + after_label;
    let target_start = close_label + 2;
    if !line[close_label + 1..].starts_with('(') || target_start >= line.len() {
        return None;
    }
    let target_end = line[target_start..].find(')')? + target_start;
    let label = &line[after_label..close_label];
    let target = &line[target_start..target_end];
    (!label.is_empty() && !target.is_empty()).then(|| MarkdownSpan::Link {
        start,
        end: target_end + 1,
        label: label.to_string(),
        target: target.to_string(),
    })
}

fn next_raw_url(line: &str) -> Option<MarkdownSpan> {
    let start = match (line.find("https://"), line.find("http://")) {
        (Some(https), Some(http)) => https.min(http),
        (Some(https), None) => https,
        (None, Some(http)) => http,
        (None, None) => return None,
    };
    let mut end = line[start..]
        .find(|ch: char| ch.is_whitespace())
        .map_or(line.len(), |offset| start + offset);
    while end > start
        && matches!(
            line[..end].chars().last(),
            Some('.' | ',' | ';' | ':' | '!' | '?' | ')' | ']')
        )
    {
        end -= line[..end]
            .chars()
            .last()
            .expect("end is after start")
            .len_utf8();
    }
    (end > start).then_some(MarkdownSpan::RawUrl { start, end })
}

fn has_unresolved_inline_markdown(line: &str) -> bool {
    let Some(code_ranges) = complete_delimiter_ranges(line, "`", &[]) else {
        return true;
    };
    let Some(link_ranges) = complete_link_ranges(line, &code_ranges) else {
        return true;
    };
    let ignored_ranges = [code_ranges, link_ranges].concat();

    has_unclosed_raw_url(line, &ignored_ranges)
        || complete_delimiter_ranges(line, "**", &ignored_ranges).is_none()
        || complete_delimiter_ranges(line, "*", &ignored_ranges).is_none()
        || complete_delimiter_ranges(line, "_", &ignored_ranges).is_none()
}

fn complete_link_ranges(
    line: &str,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Option<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::new();
    let mut search_from = 0;
    while let Some(start) = find_char_outside_ranges(line, '[', search_from, ignored_ranges) {
        let close_label =
            find_char_outside_ranges(line, ']', start + '['.len_utf8(), ignored_ranges)?;
        let target_start = close_label + "](".len();
        if !line[close_label + ']'.len_utf8()..].starts_with('(') {
            search_from = close_label + ']'.len_utf8();
            continue;
        }
        if target_start >= line.len() {
            return None;
        }
        let target_end = line[target_start..].find(')')? + target_start;
        if close_label == start + '['.len_utf8() || target_end == target_start {
            return None;
        }
        ranges.push(start..target_end + ')'.len_utf8());
        search_from = target_end + ')'.len_utf8();
    }
    Some(ranges)
}

fn complete_delimiter_ranges(
    line: &str,
    marker: &str,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Option<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::new();
    let mut search_from = 0;
    while let Some(start) = find_marker_outside_ranges(line, marker, search_from, ignored_ranges) {
        if marker == "*" && line[start..].starts_with("**") {
            search_from = start + marker.len();
            continue;
        }
        if !is_valid_stream_delimiter(line, marker, start) {
            search_from = start + marker.len();
            continue;
        }

        let content_start = start + marker.len();
        let mut end_search_from = content_start;
        let mut matched_end = None;
        while let Some(end) =
            find_marker_outside_ranges(line, marker, end_search_from, ignored_ranges)
        {
            if marker == "*" && line[end..].starts_with("**") {
                end_search_from = end + marker.len();
                continue;
            }
            if !is_valid_stream_delimiter(line, marker, end) {
                end_search_from = end + marker.len();
                continue;
            }
            if end > content_start {
                matched_end = Some(end);
            }
            break;
        }
        let end = matched_end?;
        ranges.push(start..end + marker.len());
        search_from = end + marker.len();
    }
    Some(ranges)
}

fn is_valid_stream_delimiter(line: &str, marker: &str, marker_start: usize) -> bool {
    let before = line[..marker_start].chars().next_back();
    let after = line[marker_start + marker.len()..].chars().next();
    if after.is_some_and(char::is_whitespace)
        || before.is_some_and(char::is_whitespace) && after.is_none()
    {
        return false;
    }
    marker != "_"
        || !matches!((before, after), (Some(before), Some(after)) if is_word_char(before) && is_word_char(after))
}

fn has_unclosed_raw_url(line: &str, ignored_ranges: &[std::ops::Range<usize>]) -> bool {
    let mut search_from = 0;
    while let Some(start) = next_raw_url_start(line, search_from) {
        if !is_inside_ranges(start, ignored_ranges)
            && !line[start..].chars().any(char::is_whitespace)
        {
            return true;
        }
        search_from = start + "http://".len();
    }
    false
}

fn next_raw_url_start(line: &str, search_from: usize) -> Option<usize> {
    ["https://", "http://"]
        .into_iter()
        .filter_map(|scheme| {
            line[search_from..]
                .find(scheme)
                .map(|index| search_from + index)
        })
        .min()
}

fn find_char_outside_ranges(
    line: &str,
    needle: char,
    search_from: usize,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Option<usize> {
    line[search_from..]
        .char_indices()
        .map(|(index, ch)| (search_from + index, ch))
        .find(|(index, ch)| *ch == needle && !is_inside_ranges(*index, ignored_ranges))
        .map(|(index, _)| index)
}

fn find_marker_outside_ranges(
    line: &str,
    marker: &str,
    search_from: usize,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Option<usize> {
    let mut current = search_from;
    while let Some(relative_index) = line[current..].find(marker) {
        let index = current + relative_index;
        if !is_inside_ranges(index, ignored_ranges) {
            return Some(index);
        }
        current = index + marker.len();
    }
    None
}

fn is_inside_ranges(index: usize, ranges: &[std::ops::Range<usize>]) -> bool {
    ranges
        .iter()
        .any(|range| range.start <= index && index < range.end)
}

fn next_delimited(line: &str, marker: &str, style: Style) -> Option<MarkdownSpan> {
    let mut search_from = 0;
    while let Some(relative_start) = line[search_from..].find(marker) {
        let start = search_from + relative_start;
        if marker == "*" && line[start..].starts_with("**") {
            search_from = start + marker.len();
            continue;
        }
        if marker == "_" && !is_valid_underscore_delimiter(line, start) {
            search_from = start + marker.len();
            continue;
        }

        let content_start = start + marker.len();
        let mut end_search_from = content_start;
        while let Some(relative_end) = line[end_search_from..].find(marker) {
            let end = end_search_from + relative_end;
            if marker == "_" && !is_valid_underscore_delimiter(line, end) {
                end_search_from = end + marker.len();
                continue;
            }
            if end > content_start {
                return Some(MarkdownSpan::Styled {
                    start,
                    marker_len: marker.len(),
                    end,
                    style,
                });
            }
            break;
        }
        search_from = content_start;
    }
    None
}

fn is_valid_underscore_delimiter(line: &str, marker_start: usize) -> bool {
    let before = line[..marker_start].chars().next_back();
    let after = line[marker_start + 1..].chars().next();
    !matches!((before, after), (Some(before), Some(after)) if is_word_char(before) && is_word_char(after))
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn markdown_inline_text(line: &str) -> String {
    markdown_inline_segments(line)
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
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

    let mut char_start = 0;
    wrap_line_at_whitespace_ranges(&text, width)
        .into_iter()
        .map(|range| {
            let char_count = text[range].chars().count();
            let char_end = char_start + char_count;
            let line = Line::from(merge_styled_chars(&chars[char_start..char_end]));
            char_start = char_end;
            line
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
        spans.push(Span::styled(
            String::new(),
            Style::default().remove_modifier(ratatui::style::Modifier::UNDERLINED),
        ));
    }
    spans
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
