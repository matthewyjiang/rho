mod entry_render;

pub(super) use entry_render::{
    apply_markdown_images, entry_lines, render_entry_with_options, TrailingBlank,
};

use super::{
    changelog_command::changelog_lines,
    composer_chrome::wrap_footer_parts,
    feed_image::{reserve_entry_image_rows, reserve_markdown_image_rows},
    first_run::SetupState,
    info_command::runtime_info_lines,
    message_render::{render_assistant_content, render_reasoning_content},
    rendered_entry::RenderedEntry,
    theme::Theme,
    Entry, FeedImage, UiPicker,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use std::borrow::Cow;

use ratatui::{
    layout::Position,
    style::{Modifier, Style},
    text::{Line, Span},
};

/// Rows outside the picker that must stay visible: history headroom, the
/// composer divider, and the statusline.
const PICKER_RESERVED_FEED_ROWS: usize = 5;

/// Stable text prefix for [`Entry::Error`] so severity survives without color.
pub(super) const ERROR_ENTRY_MARKER: &str = "error: ";

/// Shared space pad for render hot paths. Typical terminal widths fit here, so
/// padding can borrow instead of allocating `" ".repeat(n)` on every row.
const PAD_SPACES: &str = concat!(
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
);

/// Spaces of `width` columns, borrowed from [`PAD_SPACES`] when they fit.
pub(super) fn pad_spaces(width: usize) -> Cow<'static, str> {
    if width <= PAD_SPACES.len() {
        Cow::Borrowed(&PAD_SPACES[..width])
    } else {
        Cow::Owned(" ".repeat(width))
    }
}

fn push_pad_spaces(buf: &mut String, width: usize) {
    buf.push_str(&pad_spaces(width));
}

/// Rows the inline list picker spends on its own chrome, matching what
/// `list_picker_lines` emits around the item rows.
fn list_picker_chrome_rows(picker: &UiPicker, footer_rows: usize) -> usize {
    // filter + blank + count + blank + footer lines, plus detail + blank when shown.
    4 + footer_rows + if picker.has_item_details() { 2 } else { 0 }
}

/// Item rows a picker can list in a `viewport_height` row terminal.
///
/// The list grows with the terminal instead of staying at the number that fits
/// the default height fallback, so a tall window shows a long model or session
/// list without scrolling.
fn picker_visible_item_cap(picker: &UiPicker, viewport_height: usize, footer_rows: usize) -> usize {
    viewport_height
        .saturating_sub(list_picker_chrome_rows(picker, footer_rows))
        .saturating_sub(PICKER_RESERVED_FEED_ROWS)
        .max(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LineFill {
    Natural,
    PadToWidth,
}

impl LineFill {
    pub(super) fn pads_to_width(self) -> bool {
        matches!(self, Self::PadToWidth)
    }
}

pub(super) fn session_header_lines(
    update_notice: Option<&str>,
    setup: SetupState,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled("rho", Theme::brand()),
            Span::styled("  v", Theme::dim()),
            Span::styled(super::smoke_injection::display_version(), Theme::success()),
        ]),
    ];
    if let Some(notice) = update_notice {
        // Match the brand line's leading space so the notice lines up under "rho".
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                truncate_one_line(notice, width.saturating_sub(1)),
                Theme::warning(),
            ),
        ]));
    }
    if let Some(headline) = setup.headline() {
        lines.push(Line::raw(""));
        lines.push(Line::from(headline));
    }
    lines.push(Line::raw(""));
    push_session_header_hints(&mut lines, setup, width);
    lines.push(Line::raw(""));
    lines
}

fn push_session_header_hints(lines: &mut Vec<Line<'static>>, setup: SetupState, width: usize) {
    for hint in setup.hints() {
        lines.push(Line::from(Span::styled(
            truncate_one_line(hint.text, width),
            hint.style(),
        )));
    }
}

pub(super) fn picker_lines(
    picker: &UiPicker,
    width: usize,
    viewport_height: usize,
) -> Vec<Line<'static>> {
    list_picker_lines(picker, width, viewport_height)
}

fn list_picker_lines(
    picker: &UiPicker,
    width: usize,
    viewport_height: usize,
) -> Vec<Line<'static>> {
    let footer_text = list_picker_footer_text(picker, width);
    let footer_lines = list_picker_footer_lines(&footer_text, width);
    let item_cap = picker_visible_item_cap(picker, viewport_height, footer_text.len());
    let matching_indices = picker.matching_indices();
    let mut lines = Vec::with_capacity(item_cap + 7);
    lines.push(picker_filter_line(picker, width));
    lines.push(Line::raw(""));

    if matching_indices.is_empty() {
        lines.push(styled_line(
            truncate_one_line(&format!("  {}", picker.empty_match_message()), width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
        lines.push(Line::raw(""));
        lines.extend(footer_lines);
        return lines;
    }

    let row_layout = super::picker_rows::RowLayout {
        width,
        width_mode: super::picker_rows::RowWidthMode::AlignedColumn(
            super::picker_rows::label_column_width(&picker.items, width),
        ),
        show_badges: true,
        show_preview: true,
        fill: LineFill::Natural,
    };
    let rows = super::picker_rows::picker_item_rows(
        &picker.items,
        &matching_indices,
        picker.selected,
        row_layout,
        /*hovered_row*/ None,
    );
    let start = super::picker_rows::scroll_window_start(rows.selected_row, item_cap);
    lines.extend(rows.rows.into_iter().skip(start).take(item_cap));

    let selected_position = matching_indices
        .iter()
        .position(|index| *index == picker.selected)
        .unwrap_or(0);
    lines.push(styled_line(
        truncate_one_line(
            &format!("  ({}/{})", selected_position + 1, matching_indices.len()),
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    lines.push(Line::raw(""));
    if picker.has_item_details() {
        let detail = picker
            .selected_item()
            .and_then(|item| item.detail.as_deref())
            .unwrap_or_default();
        let detail = truncate_one_line(detail, width.saturating_sub(2));
        let detail = if width > 2 {
            format!("  {detail}")
        } else {
            truncate_one_line(&detail, width)
        };
        lines.push(styled_line(detail, width, Theme::dim(), LineFill::Natural));
        lines.push(Line::raw(""));
    }
    lines.extend(footer_lines);
    lines
}

fn list_picker_footer_text(picker: &UiPicker, width: usize) -> Vec<String> {
    let parts = picker.list_footer_parts();
    let inner_width = width.saturating_sub(2);
    let indent = width > 2;
    wrap_footer_parts(parts.iter().map(String::as_str), inner_width)
        .into_iter()
        .map(|line| if indent { format!("  {line}") } else { line })
        .collect()
}

fn list_picker_footer_lines(footer_text: &[String], width: usize) -> Vec<Line<'static>> {
    footer_text
        .iter()
        .map(|line| {
            styled_line(
                truncate_one_line(line, width),
                width,
                Theme::dim(),
                LineFill::Natural,
            )
        })
        .collect()
}

fn picker_filter_line(picker: &UiPicker, width: usize) -> Line<'static> {
    if width <= 1 {
        return Line::from(Span::styled(">", Theme::text_strong()));
    }

    Line::from(vec![
        Span::styled(">", Theme::text_strong()),
        Span::raw(" "),
        Span::styled(
            truncate_one_line(&picker.filter, width.saturating_sub(2)),
            Theme::text_strong(),
        ),
    ])
}

pub(super) fn truncate_one_line(text: &str, width: usize) -> String {
    // Fast path: no newlines means we skip the replace allocation entirely.
    if !text.contains('\n') {
        if UnicodeWidthStr::width(text) <= width {
            return text.to_string();
        }
        if width <= 1 {
            return "…".chars().take(width).collect();
        }
        return format!("{}…", truncate_to_display_width(text, width - 1));
    }

    let normalized = text.replace('\n', " ");
    if UnicodeWidthStr::width(normalized.as_str()) <= width {
        return normalized;
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    format!("{}…", truncate_to_display_width(&normalized, width - 1))
}

/// Truncate from the front, keeping the end of `text` with a leading ellipsis.
pub(super) fn truncate_keep_end(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    // Fast path: no newlines means we skip the replace allocation entirely.
    if !text.contains('\n') {
        if display_width(text) <= width {
            return text.to_string();
        }
        return truncate_keep_end_owned(text, width);
    }
    let normalized = text.replace('\n', " ");
    if display_width(&normalized) <= width {
        return normalized;
    }
    truncate_keep_end_owned(&normalized, width)
}

fn truncate_keep_end_owned(text: &str, width: usize) -> String {
    if width <= 1 {
        return "…".chars().take(width).collect();
    }

    let target = width - 1;
    let mut start = text.len();
    let mut used = 0usize;
    for (index, ch) in text.char_indices().rev() {
        let ch_width = char_display_width(ch);
        if used + ch_width > target {
            break;
        }
        used += ch_width;
        start = index;
    }
    format!("…{}", &text[start..])
}

pub(super) fn display_width(text: &str) -> usize {
    text.split(char::is_control)
        .map(UnicodeWidthStr::width)
        .sum()
}

pub(super) fn char_display_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

pub(super) fn truncate_to_display_width(text: &str, max_width: usize) -> Cow<'_, str> {
    if display_width(text) <= max_width {
        return Cow::Borrowed(text);
    }
    let mut end = 0;
    let mut width = 0;
    for (index, ch) in text.char_indices() {
        let ch_width = char_display_width(ch);
        if width + ch_width > max_width {
            break;
        }
        width += ch_width;
        end = index + ch.len_utf8();
    }
    Cow::Owned(text[..end].to_string())
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CompleteVisualPrefix {
    pub(super) byte_index: usize,
    pub(super) ends_with_wrap: bool,
}

#[cfg(test)]
pub(super) fn complete_visual_prefix(text: &str, width: usize) -> CompleteVisualPrefix {
    complete_visual_line_ends(text, width)
        .last()
        .copied()
        .map(|(index, ch)| CompleteVisualPrefix {
            byte_index: index,
            ends_with_wrap: ch != '\n',
        })
        .unwrap_or_default()
}

#[cfg(test)]
fn complete_visual_prefix_byte_index(text: &str, width: usize) -> usize {
    complete_visual_prefix(text, width).byte_index
}

#[cfg(test)]
fn complete_visual_line_ends(text: &str, width: usize) -> Vec<(usize, char)> {
    let width = width.max(1);
    let mut ends = Vec::new();
    let mut line_start = 0;

    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            ends.extend(complete_word_wrapped_line_ends(
                &text[line_start..index],
                line_start,
                width,
            ));
            ends.push((index + ch.len_utf8(), ch));
            line_start = index + ch.len_utf8();
        }
    }

    if line_start < text.len() {
        ends.extend(complete_word_wrapped_line_ends(
            &text[line_start..],
            line_start,
            width,
        ));
    }

    ends
}

#[cfg(test)]
fn complete_word_wrapped_line_ends(line: &str, offset: usize, width: usize) -> Vec<(usize, char)> {
    wrap_line_at_whitespace_ranges(line, width)
        .into_iter()
        .filter(|range| {
            range.end < line.len() || display_width(&line[range.clone()]) >= width.max(1)
        })
        .map(|range| (offset + range.end, 'x'))
        .collect()
}

/// Wrapped composer rows plus the caret position, from one soft-wrap pass.
pub(super) struct InputFrame {
    pub(super) lines: Vec<Line<'static>>,
    /// Caret column in display columns and row among `lines`. Excludes any
    /// prompt prefix or chrome the caller stacks above the text.
    pub(super) cursor: Position,
}

/// Composer text rows plus the caret, derived from the same wrap so layout
/// and cursor paint can never disagree or pay for a second wrap.
pub(super) fn input_frame(
    input: &str,
    cursor: usize,
    width: usize,
    highlighted_range: Option<std::ops::Range<usize>>,
) -> InputFrame {
    let visual_lines = editable_input_visual_lines(input, width);
    let caret = visual_caret_position(&visual_lines, input, cursor);
    let mut lines = Vec::new();
    // Walk `input` in lockstep with the visual lines. Composer soft wrap preserves
    // every character (including break spaces), so one pass replaces a per-frame
    // `Vec<char>` of the whole composer.
    let mut input_chars = input.chars().peekable();
    let mut input_cursor = 0;
    for (line_index, visual_line) in visual_lines.iter().enumerate() {
        if line_index > 0 && input_chars.peek() == Some(&'\n') {
            input_chars.next();
            input_cursor += 1;
        }
        let mut spans = Vec::new();
        let mut span_text = String::new();
        let mut span_highlighted = false;
        for character in visual_line.chars() {
            let highlighted = highlighted_range
                .as_ref()
                .is_some_and(|range| range.contains(&input_cursor));
            input_chars.next();
            input_cursor += 1;
            if !span_text.is_empty() && highlighted != span_highlighted {
                let style = if span_highlighted {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                spans.push(Span::styled(std::mem::take(&mut span_text), style));
            }
            span_highlighted = highlighted;
            span_text.push(character);
        }
        if !span_text.is_empty() {
            let style = if span_highlighted {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            spans.push(Span::styled(span_text, style));
        }
        lines.push(Line::from(spans));
    }
    InputFrame {
        lines,
        cursor: caret,
    }
}

/// Row and display column of char index `cursor` across `visual_lines`.
///
/// The lines partition `input` with hard newlines between rows they split, so
/// the caret lands on the row that paints the character under the cursor: a
/// cursor on a line end stays there when the break is a hard newline (or end
/// of input) and falls to the next row when the line soft-wrapped.
pub(super) fn visual_caret_position(
    visual_lines: &[String],
    input: &str,
    cursor: usize,
) -> Position {
    let mut chars = input.chars().peekable();
    let mut line_start = 0usize;
    for (row, line) in visual_lines.iter().enumerate() {
        let len = line.chars().count();
        let line_end = line_start + len;
        // Consume this row's source chars so `chars.peek()` is the row break.
        for _ in 0..len {
            chars.next();
        }
        let next_is_newline = chars.peek() == Some(&'\n');
        let last_row = row + 1 == visual_lines.len();
        if cursor < line_end || (cursor == line_end && (last_row || next_is_newline)) {
            let column_byte = line
                .char_indices()
                .nth(cursor - line_start)
                .map_or(line.len(), |(byte, _)| byte);
            return Position {
                x: display_width(&line[..column_byte]) as u16,
                y: row as u16,
            };
        }
        if next_is_newline {
            chars.next();
            line_start = line_end + 1;
        } else {
            line_start = line_end;
        }
    }
    Position {
        x: 0,
        y: visual_lines.len().saturating_sub(1) as u16,
    }
}

pub(super) fn char_prefix_display_width(value: &str, cursor: usize) -> usize {
    value
        .chars()
        .take(cursor)
        .map(char_display_width)
        .sum::<usize>()
}

pub(super) fn input_cursor_index_on_visual_line(
    input: &str,
    visual_lines: &[String],
    target_row: usize,
    target_column: usize,
) -> usize {
    // Walk `input` once alongside the visual lines. Re-seeking with `nth` per row
    // made cursor movement quadratic in the length of the composer text.
    let mut chars = input.chars().peekable();
    let mut line_start = 0;
    for line in visual_lines.iter().take(target_row) {
        let consumed = line.chars().count();
        for _ in 0..consumed {
            chars.next();
        }
        line_start += consumed;
        // A hard newline sits between visual lines and belongs to neither.
        if chars.peek() == Some(&'\n') {
            chars.next();
            line_start += 1;
        }
    }

    let mut cursor = line_start;
    let mut column = 0;
    if let Some(line) = visual_lines.get(target_row) {
        for ch in line.chars() {
            let next_column = column + char_display_width(ch);
            if next_column > target_column {
                break;
            }
            column = next_column;
            cursor += 1;
        }
    }
    cursor
}

/// Map a visual row/column inside the editable composer to a character index.
pub(super) fn input_char_index_at_position(
    input: &str,
    width: usize,
    row: usize,
    column: usize,
) -> usize {
    let visual_lines = editable_input_visual_lines(input, width);
    if visual_lines.is_empty() {
        return 0;
    }
    let row = row.min(visual_lines.len() - 1);
    input_cursor_index_on_visual_line(input, &visual_lines, row, column)
}

pub(super) fn editable_input_visual_lines(input: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = input_visual_lines(input, width);
    if !input.is_empty()
        && lines
            .last()
            .is_some_and(|line| display_width(line) >= width)
    {
        lines.push(String::new());
    }
    lines
}

pub(super) fn input_visual_lines(input: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw_line in input.split('\n') {
        // Covering soft-wrap: every source character stays on some visual line so
        // cursor/highlight walks can lockstep with `input` chars.
        let wrapped = wrap_line_at_whitespace_ranges(raw_line, width)
            .into_iter()
            .map(|range| raw_line[range].to_string())
            .collect::<Vec<_>>();
        if wrapped.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrapped);
        }
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

pub(super) fn tool_entry_lines(
    tool: &super::ToolEntry,
    width: usize,
    max_tool_output_lines: usize,
    max_image_height: u16,
) -> Vec<Line<'static>> {
    super::tool_card_render::tool_entry_lines(tool, width, max_tool_output_lines, max_image_height)
}

fn render_non_assistant_entry(
    lines: &mut Vec<Line<'static>>,
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
) {
    match entry {
        Entry::User(text) => push_wrapped_text(
            lines,
            text,
            width,
            Theme::user_message(),
            LineFill::PadToWidth,
        ),
        Entry::Assistant(_) | Entry::Reasoning(_) => {
            unreachable!("assistant and reasoning entries are rendered as markdown")
        }
        Entry::Tool(tool) => {
            super::tool_card_render::push_tool_card(
                lines,
                &tool.card,
                width,
                max_tool_output_lines,
                tool.expanded,
                super::tool_card_render::live_shell_elapsed(tool),
            );
        }
        Entry::Notice(text) => {
            push_wrapped_text(lines, text, width, Theme::dim_italic(), LineFill::Natural)
        }
        Entry::RuntimeInfo(info) => lines.extend(runtime_info_lines(info, width)),
        Entry::Changelog(display) => lines.extend(changelog_lines(display, width)),
        Entry::Error(text) => {
            // Text marker keeps severity readable when color is flattened
            // (monochrome themes, colorblind setups, NO_COLOR-like terminals).
            let marked = format!("{ERROR_ENTRY_MARKER}{text}");
            push_wrapped_text(lines, &marked, width, Theme::error(), LineFill::Natural)
        }
    }
}

pub(super) fn push_wrapped_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    fill: LineFill,
) {
    push_wrapped_text_with(lines, text, width, style, fill, wrap_line_at_whitespace);
}

pub(super) fn push_wrapped_text_with(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    fill: LineFill,
    wrap_line: fn(&str, usize) -> Vec<&str>,
) {
    let width = width.max(1);
    let mut emitted = false;
    for raw_line in text.lines() {
        let chunks = wrap_line(raw_line, width);
        for chunk in chunks {
            lines.push(styled_line(chunk.to_string(), width, style, fill));
            emitted = true;
        }
    }

    if !emitted {
        lines.push(styled_line(String::new(), width, style, fill));
    }
}

pub(super) fn styled_line(
    mut text: String,
    width: usize,
    style: Style,
    fill: LineFill,
) -> Line<'static> {
    if fill.pads_to_width() {
        let len = display_width(&text);
        if len < width {
            push_pad_spaces(&mut text, width - len);
        }
    }
    Line::from(Span::styled(text, style))
}

pub(super) fn styled_blank_line(width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled(pad_spaces(width.max(1)), style))
}

/// Word-wrap a line for display. Break-boundary whitespace is collapsed so
/// continuation rows are not indented; pure whitespace lines still wrap.
pub(super) fn wrap_line_at_whitespace(line: &str, width: usize) -> Vec<&str> {
    soft_wrap_visible_ranges(line, wrap_line_at_whitespace_ranges(line, width))
        .map(|range| &line[range])
        .collect()
}

/// Covering soft-wrap ranges: every source byte belongs to exactly one range.
///
/// Display callers that should not indent continuations must run the result
/// through [`soft_wrap_visible_ranges`]. Composer lockstep uses the covering
/// ranges directly so break spaces stay addressable.
pub(super) fn wrap_line_at_whitespace_ranges(
    line: &str,
    width: usize,
) -> Vec<std::ops::Range<usize>> {
    wrap_line_at_whitespace_ranges_with_protected_prefix(line, width, 0)
}

/// Wrap at whitespace without allowing the first break to strand a semantic prefix.
///
/// `protected_prefix_end` is a byte offset whose preceding whitespace cannot be
/// used as the first wrap point. If the following token overflows, the first
/// line is filled to `width` instead.
pub(super) fn wrap_line_at_whitespace_ranges_with_protected_prefix(
    line: &str,
    width: usize,
    protected_prefix_end: usize,
) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    if line.is_empty() {
        return std::iter::once(0..0).collect();
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    while start < line.len() {
        let mut count = 0usize;
        let mut last_fitting_split = None;
        let mut whitespace_break = None;
        let mut saw_non_whitespace = false;
        let mut overflow = false;
        let mut prefer_width_split = false;

        for (relative_index, ch) in line[start..].char_indices() {
            let ch_width = char_display_width(ch);
            if count > 0 && count + ch_width > width {
                overflow = true;
                prefer_width_split = ch.is_whitespace();
                break;
            }

            count += ch_width;
            let next = start + relative_index + ch.len_utf8();
            last_fitting_split = Some(next);
            if ch.is_whitespace() {
                if saw_non_whitespace {
                    whitespace_break = Some(next);
                }
            } else {
                saw_non_whitespace = true;
            }
        }

        if !overflow {
            ranges.push(start..line.len());
            break;
        }

        let split = if prefer_width_split
            || (start == 0 && whitespace_break.is_some_and(|split| split <= protected_prefix_end))
        {
            last_fitting_split.expect("overflow requires a fitting split")
        } else {
            whitespace_break
                .filter(|split| *split > start)
                .unwrap_or_else(|| last_fitting_split.expect("overflow requires a fitting split"))
        };
        ranges.push(start..split);
        start = split;
    }

    ranges
}

/// Collapse break-boundary whitespace from covering soft-wrap ranges for display.
///
/// After a range that contained non-whitespace, leading whitespace on the next
/// range is break padding and is dropped so the continuation is not indented.
/// Pure whitespace segments keep their spaces so blank padding still wraps.
pub(super) fn soft_wrap_visible_ranges<'a>(
    line: &'a str,
    ranges: impl IntoIterator<Item = std::ops::Range<usize>> + 'a,
) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
    let mut prev_had_non_whitespace = false;
    ranges.into_iter().filter_map(move |range| {
        let end = range.end;
        let mut start = range.start;
        if prev_had_non_whitespace {
            while start < end {
                let ch = line[start..].chars().next().expect("start < end");
                if !ch.is_whitespace() {
                    break;
                }
                start += ch.len_utf8();
            }
            if start >= end {
                return None;
            }
        }
        prev_had_non_whitespace = line[start..end].chars().any(|ch| !ch.is_whitespace());
        Some(start..end)
    })
}

/// Hard-wrap `text` into display-width columns as byte ranges into `text`.
///
/// Empty input yields one empty range. Wide characters are never split. A
/// chunk that exactly fills `width` breaks after it.
pub(super) fn hard_wrap_ranges(text: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![std::ops::Range { start: 0, end: 0 }];
    }
    let mut ranges = Vec::new();
    let mut chunk_start = 0usize;
    let mut offset = 0usize;
    let mut current_width = 0usize;
    for ch in text.chars() {
        let ch_width = char_display_width(ch);
        if current_width > 0 && current_width + ch_width > width {
            ranges.push(chunk_start..offset);
            chunk_start = offset;
            current_width = 0;
        }
        offset += ch.len_utf8();
        current_width += ch_width;
        if current_width >= width {
            ranges.push(chunk_start..offset);
            chunk_start = offset;
            current_width = 0;
        }
    }
    if chunk_start < text.len() {
        ranges.push(chunk_start..text.len());
    }
    ranges
}

pub(super) fn wrap_line_hard(line: &str, width: usize) -> Vec<&str> {
    hard_wrap_ranges(line, width)
        .into_iter()
        .map(|range| &line[range])
        .collect()
}

/// Hard-wrap a pre-styled line at display columns, preserving span styles.
///
/// `text` must be the concatenation of `spans` contents. Empty input yields one
/// empty span row using `empty_style`.
pub(super) fn hard_wrap_styled_spans(
    text: &str,
    spans: &[Span<'static>],
    width: usize,
    empty_style: Style,
) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    hard_wrap_ranges(text, width)
        .into_iter()
        .map(|range| {
            let chunk = slice_spans_by_bytes(spans, range.start, range.end);
            if chunk.is_empty() {
                vec![Span::styled(String::new(), empty_style)]
            } else {
                chunk
            }
        })
        .collect()
}

/// Display width of concatenated styled spans.
pub(super) fn spans_display_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

/// Slice concatenated spans by byte offsets into their joined UTF-8 text.
pub(super) fn slice_spans_by_bytes(
    spans: &[Span<'static>],
    start: usize,
    end: usize,
) -> Vec<Span<'static>> {
    if start >= end {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    for span in spans {
        let content = span.content.as_ref();
        let span_start = offset;
        let span_end = offset + content.len();
        offset = span_end;
        if span_end <= start || span_start >= end {
            continue;
        }
        let from = start.saturating_sub(span_start);
        let to = (end - span_start).min(content.len());
        if from >= to {
            continue;
        }
        // Ranges come from the concatenated UTF-8 text, so byte edges are char edges.
        out.push(Span::styled(content[from..to].to_string(), span.style));
    }
    out
}

/// Width left for content after [`pad_display_line`] takes a column on each side.
pub(super) fn padded_content_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

/// Indent a rendered line by one column on each side.
///
/// Edge spaces keep the leading span's colors (so user-message backgrounds
/// still fill the gutter) but drop underline. Sampling the content style with
/// underline left bare URLs and other link lines drawing a one-cell underline
/// past the real text.
pub(super) fn pad_display_line(line: Line<'static>) -> Line<'static> {
    let edge_style = line
        .spans
        .first()
        .map(|span| chrome_edge_style(span.style))
        .unwrap_or_default();
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    spans.push(Span::styled(" ", edge_style));
    spans.extend(line.spans);
    spans.push(Span::styled(" ", edge_style));
    Line::from(spans)
}

/// Gutter / spacer chrome may borrow content colors but must not carry
/// underline into empty cells next to links.
pub(super) fn chrome_edge_style(style: Style) -> Style {
    style.remove_modifier(Modifier::UNDERLINED)
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
