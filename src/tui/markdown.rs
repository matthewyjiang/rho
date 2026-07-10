use ratatui::{
    style::Style,
    text::{Line, Span},
};

mod table;

#[cfg(test)]
#[path = "markdown/table_tests.rs"]
mod table_tests;

use super::{
    render::{char_display_width, display_width, wrap_line_at_whitespace_ranges, wrap_line_hard},
    theme::Theme,
};

pub(super) fn push_wrapped_markdown(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    in_code_block: &mut bool,
) {
    lines.extend(render_markdown(text, width, in_code_block).lines);
}

pub(super) fn push_wrapped_markdown_without_copy_button(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    in_code_block: &mut bool,
) {
    lines.extend(
        render_markdown_with_copy_button(text, width, in_code_block, CodeBlockCopyButton::Hidden)
            .lines,
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

fn render_markdown_with_copy_button(
    text: &str,
    width: usize,
    in_code_block: &mut bool,
    copy_button: CodeBlockCopyButton,
) -> RenderedMarkdown {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut code_blocks = Vec::new();
    let mut active_code_block: Option<(usize, std::ops::Range<usize>, Vec<&str>)> = None;

    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut line_index = 0;
    while line_index < raw_lines.len() {
        let raw_line = raw_lines[line_index];
        let code_fence = raw_line.trim_start().starts_with("```");
        if code_fence {
            if *in_code_block {
                lines.push(code_block_border(width, '╰', copy_button));
                if let Some((top_line, copy_columns, content)) = active_code_block.take() {
                    code_blocks.push(MarkdownCodeBlock {
                        top_line,
                        copy_columns,
                        text: content.join("\n"),
                    });
                }
            } else {
                let top_line = lines.len();
                lines.push(code_block_border(width, '╭', copy_button));
                if copy_button == CodeBlockCopyButton::Visible {
                    if let Some(copy_columns) = code_block_copy_columns(width) {
                        active_code_block = Some((top_line, copy_columns, Vec::new()));
                    }
                }
            }
            *in_code_block = !*in_code_block;
            line_index += 1;
            continue;
        }

        if *in_code_block {
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

        if is_markdown_divider(raw_line) {
            lines.push(markdown_divider(width));
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

    RenderedMarkdown { lines, code_blocks }
}

pub(super) fn markdown_preview_width(text: &str, width: usize, in_code_block: bool) -> usize {
    let current_line_start = text.rfind('\n').map_or(0, |index| index + '\n'.len_utf8());
    let current_line_in_code_block =
        line_starts_in_code_block(text, current_line_start, in_code_block);
    let current_line = &text[current_line_start..];
    if current_line.is_empty() || starts_with_code_fence_fragment(current_line) {
        return 0;
    }
    if !current_line_in_code_block && has_unresolved_inline_markdown(current_line) {
        return 0;
    }

    let mut in_code_block = in_code_block;
    markdown_lines(text, width, &mut in_code_block)
        .last()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| display_width(span.content.as_ref()))
                .sum()
        })
        .unwrap_or_default()
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MarkdownStreamPrefix {
    pub(super) byte_index: usize,
    pub(super) ends_with_wrap: bool,
}

pub(super) fn markdown_stream_prefix(
    text: &str,
    width: usize,
    in_code_block: bool,
) -> MarkdownStreamPrefix {
    let current_line_start = text.rfind('\n').map_or(0, |index| index + '\n'.len_utf8());
    let current_line_in_code_block =
        line_starts_in_code_block(text, current_line_start, in_code_block);
    let mut prefix = MarkdownStreamPrefix {
        byte_index: current_line_start,
        ends_with_wrap: false,
    };

    let current_line = &text[current_line_start..];
    if current_line.is_empty() || starts_with_code_fence_fragment(current_line) {
        return prefix;
    }

    if current_line_in_code_block {
        let complete =
            complete_hard_wrap_prefix(current_line, code_block_stream_content_width(width));
        if complete.byte_index > 0 {
            prefix.byte_index = current_line_start + complete.byte_index;
            prefix.ends_with_wrap = complete.ends_with_wrap;
        }
        return prefix;
    }

    let rendered_line = markdown_inline_text(current_line);
    let complete = complete_word_wrap_prefix(&rendered_line, width);
    if complete.byte_index == 0 {
        return prefix;
    }

    let rendered_prefix = &rendered_line[..complete.byte_index];
    for candidate in current_line
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(current_line.len()))
        .skip(1)
    {
        let absolute_candidate = current_line_start + candidate;
        if markdown_safe_prefix_len(text, absolute_candidate, in_code_block) != absolute_candidate {
            continue;
        }
        let candidate_source = &current_line[..candidate];
        let candidate_rendered = markdown_inline_text(candidate_source);
        if candidate_rendered == rendered_prefix {
            prefix.byte_index = absolute_candidate;
            prefix.ends_with_wrap = complete.ends_with_wrap;
        } else if candidate_source.len() != candidate_rendered.len()
            && !candidate_source
                .chars()
                .last()
                .is_some_and(char::is_whitespace)
            && candidate_rendered.starts_with(rendered_prefix)
        {
            prefix.byte_index = absolute_candidate;
            prefix.ends_with_wrap = false;
        }
    }

    prefix
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CompleteStreamPrefix {
    byte_index: usize,
    ends_with_wrap: bool,
}

fn complete_word_wrap_prefix(text: &str, width: usize) -> CompleteStreamPrefix {
    wrap_line_at_whitespace_ranges(text, width)
        .into_iter()
        .rfind(|range| {
            range.end < text.len() || display_width(&text[range.clone()]) >= width.max(1)
        })
        .map(|range| CompleteStreamPrefix {
            byte_index: range.end,
            ends_with_wrap: true,
        })
        .unwrap_or_default()
}

fn complete_hard_wrap_prefix(text: &str, width: usize) -> CompleteStreamPrefix {
    let width = width.max(1);
    let mut line_width = 0;
    let mut last_complete = 0;
    for (index, ch) in text.char_indices() {
        let ch_width = char_display_width(ch);
        if line_width > 0 && line_width + ch_width > width {
            last_complete = index;
            line_width = 0;
        }
        line_width += ch_width;
        let next = index + ch.len_utf8();
        if line_width >= width {
            last_complete = next;
            line_width = 0;
        }
    }

    if last_complete == 0 {
        CompleteStreamPrefix::default()
    } else {
        CompleteStreamPrefix {
            byte_index: last_complete,
            ends_with_wrap: true,
        }
    }
}

fn markdown_safe_prefix_len(text: &str, candidate_byte_index: usize, in_code_block: bool) -> usize {
    let candidate_byte_index = candidate_byte_index.min(text.len());
    let prefix = &text[..candidate_byte_index];
    let current_line_start = prefix
        .rfind('\n')
        .map_or(0, |index| index + '\n'.len_utf8());
    let current_line_in_code_block =
        line_starts_in_code_block(prefix, current_line_start, in_code_block);

    let current_line = &prefix[current_line_start..];
    if current_line.is_empty() {
        return candidate_byte_index;
    }
    if starts_with_code_fence_fragment(current_line) {
        return current_line_start;
    }
    if current_line_in_code_block || !has_unresolved_inline_markdown(current_line) {
        candidate_byte_index
    } else {
        current_line_start
    }
}

fn line_starts_in_code_block(text: &str, line_start: usize, in_code_block: bool) -> bool {
    let mut current_line_in_code_block = in_code_block;
    for complete_line in text[..line_start].split_inclusive('\n') {
        if complete_line
            .trim_end_matches('\n')
            .trim_start()
            .starts_with("```")
        {
            current_line_in_code_block = !current_line_in_code_block;
        }
    }
    current_line_in_code_block
}

fn code_block_stream_content_width(width: usize) -> usize {
    let width = width.max(1);
    match width {
        1 => 1,
        2 | 3 => width - 1,
        width => width - 4,
    }
}

fn starts_with_code_fence_fragment(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && (trimmed.starts_with("```") || (trimmed.len() < 3 && "```".starts_with(trimmed)))
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
) -> Line<'static> {
    let width = width.max(1);
    let style = Theme::markdown_code_block();
    if width == 1 {
        return Line::from(Span::styled(corner.to_string(), style));
    }

    let closing_corner = if corner == '╭' { '╮' } else { '╯' };
    let Some(copy_columns) = (corner == '╭' && copy_button == CodeBlockCopyButton::Visible)
        .then(|| code_block_copy_columns(width))
        .flatten()
    else {
        return Line::from(Span::styled(
            format!(
                "{corner}{}{closing_corner}",
                "─".repeat(width.saturating_sub(2))
            ),
            style,
        ));
    };
    let label = code_block_copy_label(width).unwrap_or_default();
    Line::from(vec![
        Span::styled(
            format!(
                "{corner}{}",
                "─".repeat(copy_columns.start.saturating_sub(1))
            ),
            style,
        ),
        Span::styled(label, Theme::markdown_code_copy_button(/*hovered*/ false)),
        Span::styled(closing_corner.to_string(), style),
    ])
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
            MarkdownSpan::Link { start, .. } => (*start, std::cmp::Reverse(1)),
            MarkdownSpan::RawUrl { start, .. } => (*start, std::cmp::Reverse(1)),
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
        spans.push(Span::styled(
            String::new(),
            Style::default().remove_modifier(ratatui::style::Modifier::UNDERLINED),
        ));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Modifier, Style};

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
    fn styles_inline_code_bold_italic_and_links_without_markers() {
        let mut in_code_block = false;
        let lines = markdown_lines(
            "use `cargo test`, then **ship** the *fix*, [docs](https://example.com), and https://example.com",
            120,
            &mut in_code_block,
        );

        assert_eq!(
            line_text(&lines[0]),
            "use cargo test, then ship the fix, docs: https://example.com, and https://example.com"
        );
        let styles = line_styles(&lines[0]);
        assert!(styles.contains(&Theme::markdown_inline_code()));
        assert!(styles.contains(&Theme::markdown_bold()));
        assert!(styles.contains(&Theme::markdown_italic()));
        assert!(styles.contains(&Theme::markdown_link()));
        assert_eq!(Theme::markdown_bold().fg, None);
        assert_eq!(Theme::markdown_italic().fg, None);
        assert_eq!(Theme::markdown_link().fg, Theme::accent().fg);
        assert!(Theme::markdown_link().has_modifier(Modifier::UNDERLINED));
        assert_eq!(
            styles
                .iter()
                .filter(|style| **style == Theme::markdown_link())
                .count(),
            2
        );
    }

    #[test]
    fn preserves_underscores_inside_identifiers() {
        let mut in_code_block = false;
        let lines = markdown_lines(
            "keep foo_bar_baz literal but style _this_",
            120,
            &mut in_code_block,
        );

        assert_eq!(
            line_text(&lines[0]),
            "keep foo_bar_baz literal but style this"
        );
        assert!(line_styles(&lines[0]).contains(&Theme::markdown_italic()));
    }

    #[test]
    fn renders_code_blocks_with_closed_borders() {
        let mut in_code_block = false;
        let lines = markdown_lines("```rust\nlet x = 1;\n```", 20, &mut in_code_block);

        assert_eq!(line_text(&lines[0]), "╭──────────── COPY ╮");
        assert_eq!(line_text(&lines[1]), "│ let x = 1;       │");
        assert_eq!(line_text(&lines[2]), "╰──────────────────╯");
        assert_eq!(lines[0].spans[1].content.as_ref(), " COPY ");
        assert_eq!(
            lines[0].spans[1].style,
            Theme::markdown_code_copy_button(/*hovered*/ false)
        );
        assert_eq!(lines[1].spans[0].style, Theme::markdown_code_block());
    }

    #[test]
    fn code_block_padding_uses_display_width() {
        let mut in_code_block = false;
        let lines = markdown_lines("```\n你\n```", 6, &mut in_code_block);

        assert_eq!(line_text(&lines[1]), "│ 你 │");
        assert_eq!(display_width(&line_text(&lines[1])), 6);
    }

    #[test]
    fn code_blocks_preserve_markdown_markers_as_literal_text() {
        let mut in_code_block = false;
        let lines = markdown_lines(
            "```rust\nfn __init__() { println!(\"*ok*\"); }\n```",
            80,
            &mut in_code_block,
        );

        assert!(line_text(&lines[1]).contains("fn __init__() { println!(\"*ok*\"); }"));
        assert_eq!(line_styles(&lines[1]), vec![Theme::markdown_code_block()]);
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
