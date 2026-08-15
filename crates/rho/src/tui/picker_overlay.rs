//! Generic picker overlay line rendering.
//!
//! Feature policy (what items mean, confirm verbs, filters, chrome labels)
//! stays at call sites. This module only draws a bordered overlay with a
//! navigation list and an optional independently scrollable detail pane.
//! Detail presence is derived from item data, not a separate layout mode.
//! Every measurement comes from [`super::picker_overlay_layout`].

use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

pub(super) use super::picker_overlay_layout::clamp_overlay_scroll as clamp_detail_scroll;
use super::picker_overlay_layout::{
    picker_overlay_layout, OverlayLayout, OverlayOrientation, OverlayPanes, OverlayScrollbarState,
    BOTTOM_BORDER_ROWS, FOOTER_CHROME_ROWS, HEADER_CHROME_ROWS, SEPARATOR,
};
use super::render::wrap_line_at_whitespace;
use super::{
    display_width, styled_line, truncate_one_line, LineFill, PickerBadge, PickerBadgePlacement,
    PickerItem, Theme, UiPicker,
};

const FILTER_PREFIX: &str = " Search  > ";
const DEFAULT_NAV_LABEL: &str = " NAV";
const DEFAULT_DETAIL_LABEL: &str = " DETAILS";
const DEFAULT_NAV_KEYS_HINT: &str = "↑↓ items";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct OverlayChrome {
    pub(super) nav_label: String,
    /// Only used when the overlay has a detail pane.
    pub(super) detail_label: Option<String>,
    pub(super) nav_keys_hint: String,
}

#[derive(Clone, Debug)]
pub(super) struct OverlayFrame {
    pub(super) outer: Rect,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) cursor: Position,
}

struct OverlayChromeView<'a> {
    nav_label: &'a str,
    detail_label: &'a str,
    nav_keys_hint: &'a str,
}

struct OverlayContent<'a> {
    title: &'a str,
    filter: &'a str,
    items: &'a [PickerItem],
    matching: &'a [usize],
    selected: usize,
    selected_position: usize,
    match_count: usize,
    detail: &'a [String],
    detail_badge: Option<&'a PickerBadge>,
    show_nav_badges: bool,
    detail_focused: bool,
    detail_scroll: usize,
    /// First visible nav row, owned by the picker's scroll/follow state.
    nav_window_start: usize,
    hovered_nav_row: Option<usize>,
    footer: &'a str,
    empty_match_message: &'a str,
    chrome: OverlayChromeView<'a>,
}

pub(super) fn picker_overlay_frame(picker: &UiPicker, area: Rect) -> Option<OverlayFrame> {
    picker
        .is_overlay()
        .then(|| render_picker_overlay(picker, area))
}

pub(super) fn render_picker_overlay(picker: &UiPicker, area: Rect) -> OverlayFrame {
    let layout = picker_overlay_layout(area, picker.overlay_sizing());
    // Own footer and wrap detail before matching indices so temporary match
    // cache borrows from footer/detail helpers do not overlap.
    let detail_holder = layout
        .detail_viewport()
        .map(|viewport| picker.wrapped_detail_lines(viewport.width));
    let empty_detail = Vec::new();
    let detail: &[String] = detail_holder
        .as_ref()
        .map_or(&empty_detail, |lines| lines.as_slice());
    let footer = picker.action_footer();
    let empty_match_message = picker.empty_match_message();
    let matching = picker.matching_indices();
    let selected_position = matching
        .iter()
        .position(|index| *index == picker.selected)
        .unwrap_or(0);
    let chrome = chrome_view(picker.overlay_chrome.as_ref());
    let content = OverlayContent {
        title: &picker.title,
        filter: &picker.filter,
        items: &picker.items,
        matching: &matching,
        selected: picker.selected,
        selected_position,
        match_count: matching.len(),
        detail,
        detail_badge: picker.selected_detail_badge(),
        show_nav_badges: picker.badge_placement == PickerBadgePlacement::Navigation,
        detail_focused: picker.detail_pane_focused(),
        detail_scroll: picker.detail_scroll,
        nav_window_start: picker.nav_window_start(layout.nav_viewport_rows()),
        hovered_nav_row: picker.hovered_nav_row(),
        footer: &footer,
        empty_match_message,
        chrome,
    };
    let lines = overlay_lines(layout, content);
    let cursor = Position {
        x: layout
            .outer
            .x
            .saturating_add(1)
            .saturating_add(filter_cursor_x(picker.filter.as_str(), layout.inner_width)),
        y: layout.outer.y.saturating_add(1),
    };
    OverlayFrame {
        outer: layout.outer,
        lines,
        cursor,
    }
}

pub(super) fn overlay_detail_lines(detail: &str, detail_width: usize) -> Vec<String> {
    detail_wrapped_lines(detail, detail_width.max(1))
}

pub(super) fn filter_cursor_x(filter: &str, inner_width: usize) -> u16 {
    display_width(FILTER_PREFIX)
        .saturating_add(display_width(filter))
        .min(inner_width.saturating_sub(1)) as u16
}

fn chrome_view(chrome: Option<&OverlayChrome>) -> OverlayChromeView<'_> {
    match chrome {
        Some(chrome) => OverlayChromeView {
            nav_label: chrome.nav_label.as_str(),
            detail_label: chrome
                .detail_label
                .as_deref()
                .unwrap_or(DEFAULT_DETAIL_LABEL),
            nav_keys_hint: chrome.nav_keys_hint.as_str(),
        },
        None => OverlayChromeView {
            nav_label: DEFAULT_NAV_LABEL,
            detail_label: DEFAULT_DETAIL_LABEL,
            nav_keys_hint: DEFAULT_NAV_KEYS_HINT,
        },
    }
}

fn overlay_lines(layout: OverlayLayout, content: OverlayContent<'_>) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(layout.outer.height as usize);
    let divider_col = layout.divider_col();
    lines.push(border_line(
        layout.outer.width as usize,
        '┌',
        '┐',
        Some(content.title),
    ));
    // The array length ties the drawn header chrome to the row count geometry
    // uses to place the body, so the two cannot drift apart.
    let header_chrome: [Line<'static>; HEADER_CHROME_ROWS] = [
        content_row(
            layout.inner_width,
            filter_line(content.filter, layout.inner_width),
        ),
        horizontal_rule(layout.outer.width as usize, divider_col, '┬'),
        content_row(
            layout.inner_width,
            pane_header_line(layout, &content.chrome, content.detail_focused),
        ),
    ];
    lines.extend(header_chrome);

    let body_sections = match layout.panes {
        OverlayPanes::NavOnly { .. } => vec![nav_only_body(layout, &content)],
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::SideBySide,
            ..
        } => vec![side_by_side_body(layout, &content)],
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::Stacked,
            detail_viewport_rows: detail_rows_budget,
            nav_viewport_rows: nav_rows_budget,
            ..
        } => {
            let detail_rows = detail_viewport_rows(
                content.detail,
                content.detail_badge,
                content.detail_scroll,
                layout.inner_width,
                detail_rows_budget,
            );
            let nav_rows = nav_item_rows(&content, layout.nav_width(), nav_rows_budget);
            if detail_rows_budget > 0 && nav_rows_budget > 0 {
                vec![detail_rows, nav_rows]
            } else if detail_rows_budget > 0 {
                vec![detail_rows]
            } else {
                vec![nav_rows]
            }
        }
    };
    for (index, section) in body_sections.into_iter().enumerate() {
        if index > 0 {
            // Stacked detail/nav split: join the side borders with ├─┤.
            lines.push(horizontal_rule(layout.outer.width as usize, None, '─'));
        }
        for row in section {
            lines.push(content_row(layout.inner_width, row));
        }
    }

    while lines.len() + FOOTER_CHROME_ROWS + BOTTOM_BORDER_ROWS < layout.outer.height as usize {
        // Keep the column rule continuous through spare body rows so it meets
        // the footer junction instead of leaving a gap.
        lines.push(content_row(layout.inner_width, pane_filler_row(layout)));
    }

    let footer_chrome: [Line<'static>; FOOTER_CHROME_ROWS] = [
        horizontal_rule(layout.outer.width as usize, divider_col, '┴'),
        content_row(layout.inner_width, footer_line(layout, &content)),
    ];
    lines.extend(footer_chrome);
    lines.push(border_line(layout.outer.width as usize, '└', '┘', None));
    lines.truncate(layout.outer.height as usize);
    while lines.len() < layout.outer.height as usize {
        lines.push(Line::raw(""));
    }
    lines
}

fn side_by_side_body(layout: OverlayLayout, content: &OverlayContent<'_>) -> Vec<Line<'static>> {
    let OverlayPanes::NavAndDetail {
        nav_width,
        detail_width,
        detail_viewport_rows: detail_rows_budget,
        nav_viewport_rows,
        ..
    } = layout.panes
    else {
        return Vec::new();
    };
    let nav_rows = nav_item_rows(content, nav_width, nav_viewport_rows);
    let detail_rows = detail_viewport_rows(
        content.detail,
        content.detail_badge,
        content.detail_scroll,
        detail_width,
        detail_rows_budget,
    );
    let mut rows = Vec::with_capacity(layout.body_rows);
    for index in 0..layout.body_rows {
        let left = nav_rows
            .get(index)
            .cloned()
            .unwrap_or_else(|| padded_plain("", nav_width));
        let right = detail_rows.get(index).cloned().unwrap_or_default();
        let mut spans = left.spans;
        spans.push(Span::styled(SEPARATOR, Theme::dim()));
        spans.extend(right.spans);
        rows.push(Line::from(spans));
    }
    rows
}

fn nav_only_body(layout: OverlayLayout, content: &OverlayContent<'_>) -> Vec<Line<'static>> {
    let mut rows = nav_item_rows(content, layout.nav_width(), layout.nav_viewport_rows());
    rows.truncate(layout.body_rows);
    while rows.len() < layout.body_rows {
        rows.push(Line::raw(""));
    }
    rows
}

fn nav_item_rows(
    content: &OverlayContent<'_>,
    width: usize,
    viewport_rows: usize,
) -> Vec<Line<'static>> {
    if viewport_rows == 0 {
        return Vec::new();
    }
    if content.matching.is_empty() {
        let mut rows = vec![styled_line(
            format!("  {}", content.empty_match_message),
            width,
            Theme::dim(),
            LineFill::PadToWidth,
        )];
        rows.resize_with(viewport_rows, || padded_plain("", width));
        return rows;
    }

    let total_rows = super::picker_rows::picker_row_count(content.items, content.matching);
    let scrollbar =
        OverlayScrollbarState::nav(width, total_rows, viewport_rows, content.nav_window_start);
    let content_width = width.saturating_sub(usize::from(scrollbar.is_some()));
    let rows = super::picker_rows::picker_item_rows(
        content.items,
        content.matching,
        content.selected,
        super::picker_rows::RowLayout {
            width: content_width,
            width_mode: super::picker_rows::RowWidthMode::FillPane,
            show_badges: content.show_nav_badges,
            show_preview: false,
            fill: LineFill::PadToWidth,
        },
        content.hovered_nav_row,
    );
    // `UiPicker::nav_window_start` already clamped to this viewport's last
    // window start, so the offset needs no second clamp here.
    let start = content.nav_window_start;
    let mut visible = rows
        .rows
        .into_iter()
        .skip(start)
        .take(viewport_rows)
        .collect::<Vec<_>>();
    visible.resize_with(viewport_rows, || padded_plain("", content_width));
    if let Some(scrollbar) = scrollbar {
        append_scrollbar_column(&mut visible, scrollbar);
    }
    visible
}

/// Add a one-column track and thumb to the right edge of pane rows.
fn append_scrollbar_column(rows: &mut [Line<'static>], scrollbar: OverlayScrollbarState) {
    let thumb = scrollbar.thumb();
    for (row, line) in rows.iter_mut().enumerate() {
        line.spans
            .push(super::scrollbar::track_span(thumb, row, Theme::accent()));
    }
}

const DETAIL_BADGE_ROWS: usize = 2;

pub(super) fn detail_content_line_count(detail_lines: usize, has_badge: bool) -> usize {
    detail_lines.saturating_add(usize::from(has_badge) * DETAIL_BADGE_ROWS)
}

fn detail_badge_row(badge: &PickerBadge, width: usize) -> Line<'static> {
    let width = width.max(1);
    let label = "Status  ";
    let label_width = display_width(label);
    if label_width >= width {
        // Extremely narrow panes: drop the label and keep a truncated badge.
        return Line::from(Span::styled(
            pad_text(&badge.text, width),
            super::picker_rows::picker_badge_style(badge.tone),
        ));
    }
    let badge_budget = width.saturating_sub(label_width);
    let badge_text = truncate_one_line(&badge.text, badge_budget);
    let used_width = label_width.saturating_add(display_width(&badge_text));
    Line::from(vec![
        Span::styled(label.to_string(), Theme::dim()),
        Span::styled(
            badge_text,
            super::picker_rows::picker_badge_style(badge.tone),
        ),
        Span::raw(" ".repeat(width.saturating_sub(used_width))),
    ])
}

fn detail_viewport_rows(
    detail: &[String],
    badge: Option<&PickerBadge>,
    detail_scroll: usize,
    width: usize,
    viewport_rows: usize,
) -> Vec<Line<'static>> {
    if viewport_rows == 0 {
        return Vec::new();
    }
    let badge_rows = usize::from(badge.is_some()) * DETAIL_BADGE_ROWS;
    let line_count = detail_content_line_count(detail.len(), badge.is_some());
    let scroll = clamp_detail_scroll(detail_scroll, line_count, viewport_rows);
    let mut rows = (scroll..line_count)
        .take(viewport_rows)
        .map(|index| {
            if let Some(badge) = badge.filter(|_| index == 0) {
                return detail_badge_row(badge, width);
            }
            let text = index
                .checked_sub(badge_rows)
                .and_then(|detail_index| detail.get(detail_index))
                .map_or("", String::as_str);
            Line::from(Span::styled(pad_text(text, width), Theme::dim()))
        })
        .collect::<Vec<_>>();
    rows.resize_with(viewport_rows, || {
        Line::from(Span::styled(" ".repeat(width.max(1)), Theme::dim()))
    });
    // Fill the reserved gutter: a track when the detail overflows, blank space
    // otherwise, so the text width never changes.
    if let Some(scrollbar) = OverlayScrollbarState::detail(line_count, viewport_rows, scroll) {
        append_scrollbar_column(&mut rows, scrollbar);
    } else {
        for line in &mut rows {
            line.spans.push(Span::raw(" "));
        }
    }
    rows
}

fn detail_wrapped_lines(detail: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if detail.is_empty() {
        return vec![String::new()];
    }
    detail
        .lines()
        .flat_map(|line| {
            if line.is_empty() {
                vec![String::new()]
            } else {
                wrap_line_at_whitespace(line, width)
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            }
        })
        .collect()
}

fn footer_line(layout: OverlayLayout, content: &OverlayContent<'_>) -> Line<'static> {
    // Priority when width is tight: keep nav/page/action/match count, then a
    // short overflow cue, and only then the full detail line-range.
    let position = if content.match_count == 0 {
        "0/0".to_string()
    } else {
        format!(
            "{}/{}",
            content.selected_position.saturating_add(1),
            content.match_count
        )
    };
    let pane_hint = match layout.panes {
        OverlayPanes::NavOnly { .. } => None,
        OverlayPanes::NavAndDetail { .. } => Some("←/→ pane"),
    };
    let essential = [
        content.chrome.nav_keys_hint,
        pane_hint.unwrap_or_default(),
        "PgUp/PgDn",
        content.footer,
        position.as_str(),
    ];
    let essential = essential
        .iter()
        .copied()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let detail = match layout.panes {
        OverlayPanes::NavOnly { .. } => None,
        OverlayPanes::NavAndDetail {
            detail_viewport_rows,
            ..
        } => Some(detail_footer_status(
            content.detail.len(),
            content.detail_badge.is_some(),
            content.detail_scroll,
            detail_viewport_rows,
        )),
    };
    let text = fit_overlay_footer(&essential, detail.as_ref(), layout.inner_width);
    styled_line(text, layout.inner_width, Theme::dim(), LineFill::PadToWidth)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailOverflow {
    Below,
    Above,
}

impl DetailOverflow {
    fn label(self) -> &'static str {
        match self {
            Self::Below => "↓ more",
            Self::Above => "↑ more",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DetailFooterStatus {
    range: String,
    overflow: Option<DetailOverflow>,
}

impl DetailFooterStatus {
    fn full(&self) -> String {
        match (self.range.is_empty(), self.overflow) {
            (false, Some(overflow)) => format!("{} {}", self.range, overflow.label()),
            (false, None) => self.range.clone(),
            (true, Some(overflow)) => overflow.label().to_string(),
            (true, None) => String::new(),
        }
    }
}

fn detail_footer_status(
    detail_len: usize,
    has_badge: bool,
    detail_scroll: usize,
    detail_viewport_rows: usize,
) -> DetailFooterStatus {
    let detail_lines = detail_content_line_count(detail_len, has_badge);
    let scroll = clamp_detail_scroll(detail_scroll, detail_lines, detail_viewport_rows);
    let (range, overflow) = if detail_lines == 0 {
        (String::new(), None)
    } else {
        let visible_end = (scroll + detail_viewport_rows).min(detail_lines);
        let visible_start = scroll.saturating_add(1);
        let overflow = if detail_lines > detail_viewport_rows {
            if scroll + detail_viewport_rows < detail_lines {
                Some(DetailOverflow::Below)
            } else if scroll > 0 {
                Some(DetailOverflow::Above)
            } else {
                None
            }
        } else {
            None
        };
        (
            format!("lines {visible_start}-{visible_end} of {detail_lines}"),
            overflow,
        )
    };
    DetailFooterStatus { range, overflow }
}

fn fit_overlay_footer(
    essential: &[&str],
    detail: Option<&DetailFooterStatus>,
    width: usize,
) -> String {
    // Prefer fullest chrome first; drop low-value detail segments before hard truncation.
    let essential_text = join_footer_segments(essential.iter().copied());
    if let Some(detail) = detail {
        let full = detail.full();
        if !full.is_empty() {
            let with_range = join_footer_segments(
                essential
                    .iter()
                    .copied()
                    .chain(std::iter::once(full.as_str())),
            );
            if display_width(&with_range) <= width {
                return with_range;
            }
        }
        if let Some(overflow) = detail.overflow {
            let with_overflow = join_footer_segments(
                essential
                    .iter()
                    .copied()
                    .chain(std::iter::once(overflow.label())),
            );
            if display_width(&with_overflow) <= width {
                return with_overflow;
            }
        }
    }
    if display_width(&essential_text) <= width {
        return essential_text;
    }
    truncate_one_line(&essential_text, width)
}

fn join_footer_segments<'a>(segments: impl IntoIterator<Item = &'a str>) -> String {
    format!(" {}", super::composer_chrome::join_footer_parts(segments))
}

fn pane_header_line(
    layout: OverlayLayout,
    chrome: &OverlayChromeView<'_>,
    detail_focused: bool,
) -> Line<'static> {
    match layout.panes {
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::SideBySide,
            nav_width,
            detail_width,
            ..
        } => {
            // The focused pane keeps the strong label so ←/→ has visible effect.
            let (nav_style, detail_style) = if detail_focused {
                (Theme::dim(), Theme::text_strong())
            } else {
                (Theme::text_strong(), Theme::dim())
            };
            let left = pad_text(chrome.nav_label, nav_width);
            let right = pad_text(chrome.detail_label, detail_width);
            Line::from(vec![
                Span::styled(left, nav_style),
                Span::styled(SEPARATOR, Theme::dim()),
                Span::styled(right, detail_style),
            ])
        }
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::Stacked,
            ..
        } => styled_line(
            pad_text(chrome.detail_label, layout.inner_width),
            layout.inner_width,
            Theme::text_strong(),
            LineFill::PadToWidth,
        ),
        OverlayPanes::NavOnly { .. } => styled_line(
            pad_text(chrome.nav_label, layout.inner_width),
            layout.inner_width,
            Theme::text_strong(),
            LineFill::PadToWidth,
        ),
    }
}

fn pane_filler_row(layout: OverlayLayout) -> Line<'static> {
    match layout.panes {
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::SideBySide,
            nav_width,
            detail_width,
            ..
        } => Line::from(vec![
            Span::raw(" ".repeat(nav_width)),
            Span::styled(SEPARATOR, Theme::dim()),
            Span::raw(" ".repeat(detail_width)),
        ]),
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::Stacked,
            ..
        }
        | OverlayPanes::NavOnly { .. } => Line::raw(""),
    }
}

fn horizontal_rule(width: usize, divider_col: Option<usize>, junction: char) -> Line<'static> {
    if width == 0 {
        return Line::raw("");
    }
    if width == 1 {
        return Line::from(Span::styled("├".to_string(), Theme::dim()));
    }
    let mut text = String::with_capacity(width);
    text.push('├');
    for col in 1..width.saturating_sub(1) {
        if divider_col == Some(col) {
            text.push(junction);
        } else {
            text.push('─');
        }
    }
    text.push('┤');
    if display_width(&text) > width {
        text = truncate_one_line(&text, width);
    }
    Line::from(Span::styled(text, Theme::dim()))
}

fn filter_line(filter: &str, width: usize) -> Line<'static> {
    if width <= 1 {
        return Line::from(Span::styled(">", Theme::text_strong()));
    }
    let prefix = truncate_one_line(FILTER_PREFIX, width);
    let filter_width = width.saturating_sub(display_width(&prefix));
    Line::from(vec![
        Span::styled(prefix, Theme::dim()),
        Span::styled(
            truncate_one_line(filter, filter_width),
            Theme::text_strong(),
        ),
    ])
}

fn border_line(width: usize, left: char, right: char, title: Option<&str>) -> Line<'static> {
    if width == 0 {
        return Line::raw("");
    }
    if width == 1 {
        return Line::from(Span::styled(left.to_string(), Theme::dim()));
    }
    let mut text = left.to_string();
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        let label = format!(" {title} ");
        let label = truncate_one_line(&label, width.saturating_sub(2));
        text.push_str(&label);
        let fill = width.saturating_sub(display_width(&text)).saturating_sub(1);
        text.push_str(&"─".repeat(fill));
    } else {
        text.push_str(&"─".repeat(width.saturating_sub(2)));
    }
    text.push(right);
    if display_width(&text) > width {
        text = truncate_one_line(&text, width);
    }
    Line::from(Span::styled(text, Theme::dim()))
}

fn content_row(inner_width: usize, content: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::styled("│", Theme::dim())];
    let content_width = content
        .spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum::<usize>();
    spans.extend(content.spans);
    if content_width < inner_width {
        spans.push(Span::raw(" ".repeat(inner_width - content_width)));
    }
    spans.push(Span::styled("│", Theme::dim()));
    Line::from(spans)
}

fn padded_plain(text: &str, width: usize) -> Line<'static> {
    Line::from(Span::raw(pad_text(text, width)))
}

fn pad_text(text: &str, width: usize) -> String {
    let width = width.max(1);
    let text = truncate_one_line(text, width);
    let pad = width.saturating_sub(display_width(&text));
    format!("{text}{}", " ".repeat(pad))
}

#[cfg(test)]
#[path = "picker_overlay_tests.rs"]
mod tests;
