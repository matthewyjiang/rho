//! Generic navigable popup geometry and line rendering.
//!
//! Feature policy (what items mean, confirm verbs, filters) stays at call sites.
//! This module only lays out a bordered overlay with a navigation list and an
//! independently scrollable detail pane.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
};

use super::render::wrap_line_at_whitespace;
use super::{display_width, styled_line, truncate_one_line, LineFill, PickerBadgeTone, Theme};

const TWO_COLUMN_MIN_INNER_WIDTH: usize = 60;
const MIN_NAV_WIDTH: usize = 14;
const MAX_NAV_WIDTH: usize = 28;
const SEPARATOR: &str = " │ ";
/// Rows consumed inside the border: search, divider, pane header, status divider, footer.
const INNER_CHROME_ROWS: usize = 5;
const FILTER_PREFIX: &str = " Search  > ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NavigablePopupOrientation {
    SideBySide,
    Stacked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NavigablePopupItem {
    pub(super) label: String,
    pub(super) badge: Option<(String, PickerBadgeTone)>,
    pub(super) selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NavigablePopupLayout {
    pub(super) outer: Rect,
    pub(super) inner_width: usize,
    pub(super) inner_height: usize,
    pub(super) orientation: NavigablePopupOrientation,
    pub(super) body_rows: usize,
    pub(super) nav_width: usize,
    pub(super) detail_width: usize,
    pub(super) detail_viewport_rows: usize,
    pub(super) nav_viewport_rows: usize,
}

#[derive(Clone, Debug)]
pub(super) struct NavigablePopupContent<'a> {
    pub(super) title: &'a str,
    pub(super) filter: &'a str,
    pub(super) items: &'a [NavigablePopupItem],
    pub(super) selected_position: usize,
    pub(super) match_count: usize,
    pub(super) detail: &'a [String],
    pub(super) detail_scroll: usize,
    pub(super) footer: &'a str,
}

pub(super) fn navigable_popup_outer_rect(area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }

    let horizontal_margin = ((area.width as usize) / 20).clamp(1, 4) as u16;
    let vertical_margin = ((area.height as usize) / 12).clamp(1, 3) as u16;
    let width = area
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2))
        .max(1);
    let height = area
        .height
        .saturating_sub(vertical_margin.saturating_mul(2))
        .max(1);
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    Rect::new(x, y, width, height)
}

pub(super) fn navigable_popup_layout(outer: Rect) -> NavigablePopupLayout {
    let outer_width = outer.width as usize;
    let outer_height = outer.height as usize;
    let inner_width = outer_width.saturating_sub(2).max(1);
    let inner_height = outer_height.saturating_sub(2).max(1);
    let body_rows = inner_height.saturating_sub(INNER_CHROME_ROWS).max(1);
    let orientation = if inner_width < TWO_COLUMN_MIN_INNER_WIDTH {
        NavigablePopupOrientation::Stacked
    } else {
        NavigablePopupOrientation::SideBySide
    };

    let (nav_width, detail_width, detail_viewport_rows, nav_viewport_rows) = match orientation {
        NavigablePopupOrientation::SideBySide => {
            let nav_width = ((inner_width * 30) / 100).clamp(MIN_NAV_WIDTH, MAX_NAV_WIDTH);
            let separator_width = display_width(SEPARATOR);
            let detail_width = inner_width
                .saturating_sub(nav_width)
                .saturating_sub(separator_width)
                .max(1);
            (nav_width, detail_width, body_rows, body_rows)
        }
        NavigablePopupOrientation::Stacked => {
            let detail_viewport_rows = (body_rows.saturating_mul(3) / 5)
                .max(2)
                .min(body_rows.saturating_sub(1));
            let nav_viewport_rows = body_rows.saturating_sub(detail_viewport_rows);
            (
                inner_width,
                inner_width,
                detail_viewport_rows,
                nav_viewport_rows,
            )
        }
    };

    NavigablePopupLayout {
        outer,
        inner_width,
        inner_height,
        orientation,
        body_rows,
        nav_width,
        detail_width,
        detail_viewport_rows,
        nav_viewport_rows,
    }
}

pub(super) fn navigable_popup_detail_lines(detail: &str, detail_width: usize) -> Vec<String> {
    detail_wrapped_lines(detail, detail_width.max(1))
}

pub(super) fn clamp_detail_scroll(
    detail_scroll: usize,
    detail_line_count: usize,
    viewport_rows: usize,
) -> usize {
    let max_scroll = detail_line_count.saturating_sub(viewport_rows.max(1));
    detail_scroll.min(max_scroll)
}

pub(super) fn navigable_popup_lines(
    layout: NavigablePopupLayout,
    content: NavigablePopupContent<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(layout.outer.height as usize);
    lines.push(border_line(
        layout.outer.width as usize,
        '╔',
        '╗',
        Some(content.title),
    ));
    lines.push(content_row(
        layout.inner_width,
        filter_line(content.filter, layout.inner_width),
    ));
    lines.push(horizontal_rule(layout.outer.width as usize));
    lines.push(content_row(layout.inner_width, pane_header_line(layout)));

    let body = match layout.orientation {
        NavigablePopupOrientation::SideBySide => side_by_side_body(layout, &content),
        NavigablePopupOrientation::Stacked => stacked_body(layout, &content),
    };
    for row in body {
        lines.push(content_row(layout.inner_width, row));
    }

    while lines.len() + 3 < layout.outer.height as usize {
        lines.push(content_row(layout.inner_width, Line::raw("")));
    }

    lines.push(horizontal_rule(layout.outer.width as usize));
    lines.push(content_row(
        layout.inner_width,
        footer_line(layout, &content),
    ));
    lines.push(border_line(layout.outer.width as usize, '╚', '╝', None));
    lines.truncate(layout.outer.height as usize);
    while lines.len() < layout.outer.height as usize {
        lines.push(Line::raw(""));
    }
    lines
}

pub(super) fn navigable_popup_filter_cursor_x(filter: &str, inner_width: usize) -> u16 {
    display_width(FILTER_PREFIX)
        .saturating_add(display_width(filter))
        .min(inner_width.saturating_sub(1)) as u16
}

fn side_by_side_body(
    layout: NavigablePopupLayout,
    content: &NavigablePopupContent<'_>,
) -> Vec<Line<'static>> {
    let nav_rows = nav_item_rows(
        content.items,
        content.selected_position,
        layout.nav_width,
        layout.nav_viewport_rows,
    );
    let detail_rows = detail_viewport_rows(
        content.detail,
        content.detail_scroll,
        layout.detail_width,
        layout.detail_viewport_rows,
    );
    let mut rows = Vec::with_capacity(layout.body_rows);
    for index in 0..layout.body_rows {
        let left = nav_rows
            .get(index)
            .cloned()
            .unwrap_or_else(|| padded_plain("", layout.nav_width));
        let right = detail_rows.get(index).cloned().unwrap_or_default();
        let mut spans = left.spans;
        spans.push(Span::styled(SEPARATOR, Theme::dim()));
        spans.extend(right.spans);
        rows.push(Line::from(spans));
    }
    rows
}

fn stacked_body(
    layout: NavigablePopupLayout,
    content: &NavigablePopupContent<'_>,
) -> Vec<Line<'static>> {
    let mut rows = Vec::with_capacity(layout.body_rows);
    rows.extend(detail_viewport_rows(
        content.detail,
        content.detail_scroll,
        layout.detail_width,
        layout.detail_viewport_rows,
    ));
    rows.extend(nav_item_rows(
        content.items,
        content.selected_position,
        layout.nav_width,
        layout.nav_viewport_rows,
    ));
    rows.truncate(layout.body_rows);
    while rows.len() < layout.body_rows {
        rows.push(Line::raw(""));
    }
    rows
}

fn nav_item_rows(
    items: &[NavigablePopupItem],
    selected_position: usize,
    width: usize,
    viewport_rows: usize,
) -> Vec<Line<'static>> {
    if items.is_empty() || viewport_rows == 0 {
        return (0..viewport_rows).map(|_| Line::raw("")).collect();
    }

    let start = selected_position
        .saturating_add(1)
        .saturating_sub(viewport_rows);
    let mut rows = items
        .iter()
        .skip(start)
        .take(viewport_rows)
        .map(|item| nav_item_line(item, width))
        .collect::<Vec<_>>();
    rows.resize_with(viewport_rows, || padded_plain("", width));
    rows
}

fn nav_item_line(item: &NavigablePopupItem, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::raw("");
    }
    let marker = if item.selected { "→" } else { " " };
    let style = if item.selected {
        Theme::accent()
    } else {
        Theme::text()
    };
    if width == 1 {
        return Line::from(Span::styled(marker.to_string(), style));
    }

    let mut used = 2usize;
    let label_budget = width.saturating_sub(used).max(1);
    let label = truncate_one_line(&item.label, label_budget);
    let mut spans = vec![Span::styled(
        format!(
            "{marker} {label}{}",
            " ".repeat(label_budget.saturating_sub(display_width(&label)))
        ),
        style,
    )];
    used = 2 + label_budget;
    if let Some((badge_text, tone)) = &item.badge {
        let remaining = width.saturating_sub(used.saturating_add(1));
        if remaining > 1 {
            let badge = truncate_one_line(badge_text, remaining.min(16));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(badge, badge_style(*tone)));
        }
    }
    Line::from(spans)
}

fn detail_viewport_rows(
    detail: &[String],
    detail_scroll: usize,
    width: usize,
    viewport_rows: usize,
) -> Vec<Line<'static>> {
    if viewport_rows == 0 {
        return Vec::new();
    }
    let scroll = clamp_detail_scroll(detail_scroll, detail.len(), viewport_rows);
    let mut rows = detail
        .iter()
        .skip(scroll)
        .take(viewport_rows)
        .map(|line| Line::from(Span::styled(pad_text(line, width), Theme::dim())))
        .collect::<Vec<_>>();
    rows.resize_with(viewport_rows, || {
        Line::from(Span::styled(" ".repeat(width.max(1)), Theme::dim()))
    });
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
            }
        })
        .collect()
}

fn footer_line(layout: NavigablePopupLayout, content: &NavigablePopupContent<'_>) -> Line<'static> {
    let detail_lines = content.detail.len();
    let scroll = clamp_detail_scroll(
        content.detail_scroll,
        detail_lines,
        layout.detail_viewport_rows,
    );
    let visible_end = if detail_lines == 0 {
        0
    } else {
        (scroll + layout.detail_viewport_rows).min(detail_lines)
    };
    let visible_start = if detail_lines == 0 {
        0
    } else {
        scroll.saturating_add(1)
    };
    let overflow = if detail_lines > layout.detail_viewport_rows {
        if scroll + layout.detail_viewport_rows < detail_lines {
            " ↓ more"
        } else if scroll > 0 {
            " ↑ more"
        } else {
            ""
        }
    } else {
        ""
    };
    let position = if content.match_count == 0 {
        "0/0".to_string()
    } else {
        format!(
            "{}/{}",
            content.selected_position.saturating_add(1),
            content.match_count
        )
    };
    let detail_position =
        format!("lines {visible_start}-{visible_end} of {detail_lines}{overflow}");
    let text = format!(
        " ↑↓ agents · PgUp/PgDn details · Type search · {} · {position} · {detail_position}",
        content.footer
    );
    styled_line(
        truncate_one_line(&text, layout.inner_width),
        layout.inner_width,
        Theme::dim(),
        LineFill::PadToWidth,
    )
}

fn pane_header_line(layout: NavigablePopupLayout) -> Line<'static> {
    match layout.orientation {
        NavigablePopupOrientation::SideBySide => {
            let left = pad_text(" AGENTS", layout.nav_width);
            let right = pad_text(" DETAILS", layout.detail_width);
            Line::from(vec![
                Span::styled(left, Theme::text_strong()),
                Span::styled(SEPARATOR, Theme::dim()),
                Span::styled(right, Theme::text_strong()),
            ])
        }
        NavigablePopupOrientation::Stacked => styled_line(
            pad_text(" DETAILS", layout.inner_width),
            layout.inner_width,
            Theme::text_strong(),
            LineFill::PadToWidth,
        ),
    }
}

fn horizontal_rule(width: usize) -> Line<'static> {
    border_line(width, '╟', '╢', None)
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
    let fill_char = match left {
        '╔' | '╚' => '═',
        _ => '─',
    };
    let mut text = left.to_string();
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        let label = format!(" {title} ");
        let label = truncate_one_line(&label, width.saturating_sub(2));
        text.push_str(&label);
        let fill = width.saturating_sub(display_width(&text)).saturating_sub(1);
        text.push_str(&fill_char.to_string().repeat(fill));
    } else {
        text.push_str(&fill_char.to_string().repeat(width.saturating_sub(2)));
    }
    text.push(right);
    if display_width(&text) > width {
        text = truncate_one_line(&text, width);
    }
    Line::from(Span::styled(text, Theme::dim()))
}

fn content_row(inner_width: usize, content: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::styled("║", Theme::dim())];
    let content_width = content
        .spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum::<usize>();
    spans.extend(content.spans);
    if content_width < inner_width {
        spans.push(Span::raw(" ".repeat(inner_width - content_width)));
    }
    spans.push(Span::styled("║", Theme::dim()));
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

fn badge_style(tone: PickerBadgeTone) -> ratatui::style::Style {
    match tone {
        PickerBadgeTone::Internal => Theme::accent(),
        PickerBadgeTone::Selected => Theme::warning(),
        PickerBadgeTone::Favorite | PickerBadgeTone::Healthy => Theme::success(),
        PickerBadgeTone::Warning => Theme::warning(),
    }
}

#[cfg(test)]
#[path = "navigable_popup_tests.rs"]
mod tests;
