//! Single-pane overlay chrome: title, scrollable body, footer.
//!
//! Feature policy (what the body means, which keys close it) stays at call
//! sites. This module draws a bordered popup with one scrolling region and no
//! search field or detail split.

use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::{
    display_width,
    picker_overlay_layout::{clamp_overlay_scroll, OverlayScrollbarState},
    render::truncate_one_line,
    scrollbar::track_span,
    styled_line, LineFill, Theme,
};

const TOP_BORDER_ROWS: usize = 1;
const BOTTOM_BORDER_ROWS: usize = 1;
/// Footer rule + footer hint.
const FOOTER_CHROME_ROWS: usize = 2;
const INNER_CHROME_ROWS: usize = FOOTER_CHROME_ROWS;
const MIN_BODY_ROWS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OverlayPanelLayout {
    pub(super) outer: Rect,
    pub(super) inner_width: usize,
    pub(super) body_rows: usize,
}

#[derive(Clone, Debug)]
pub(super) struct OverlayPanelFrame {
    pub(super) outer: Rect,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) cursor: Position,
}

pub(super) fn overlay_panel_layout(area: Rect, body_line_count: usize) -> OverlayPanelLayout {
    layout_for_outer(outer_rect(area, body_line_count))
}

pub(super) fn render_overlay_panel(
    title: &str,
    footer: &str,
    body: &[Line<'static>],
    scroll: usize,
    area: Rect,
) -> OverlayPanelFrame {
    let layout = overlay_panel_layout(area, body.len());
    let inner_width = layout.inner_width;
    let body_rows = layout.body_rows;
    let scroll = clamp_overlay_scroll(scroll, body.len(), body_rows);
    let scrollbar = OverlayScrollbarState::detail(body.len(), body_rows, scroll);
    let content_width = inner_width.saturating_sub(usize::from(scrollbar.is_some()));

    let mut lines = Vec::with_capacity(layout.outer.height as usize);
    lines.push(border_line(
        layout.outer.width as usize,
        '┌',
        '┐',
        Some(title),
    ));

    let mut body_view = body
        .iter()
        .skip(scroll)
        .take(body_rows)
        .cloned()
        .map(|line| fit_body_line(line, content_width))
        .collect::<Vec<_>>();
    body_view.resize_with(body_rows, || padded_plain("", content_width));
    if let Some(scrollbar) = scrollbar {
        append_scrollbar_column(&mut body_view, scrollbar);
    }
    for row in body_view {
        lines.push(content_row(inner_width, row));
    }

    lines.push(horizontal_rule(layout.outer.width as usize));
    let footer_text = format!(" {footer}");
    lines.push(content_row(
        inner_width,
        styled_line(
            truncate_one_line(&footer_text, inner_width),
            inner_width,
            Theme::dim(),
            LineFill::PadToWidth,
        ),
    ));
    lines.push(border_line(layout.outer.width as usize, '└', '┘', None));
    lines.truncate(layout.outer.height as usize);
    while lines.len() < layout.outer.height as usize {
        lines.push(Line::raw(""));
    }

    let cursor_y = layout
        .outer
        .y
        .saturating_add(layout.outer.height.saturating_sub(2));
    OverlayPanelFrame {
        cursor: Position {
            x: layout.outer.x.saturating_add(2),
            y: cursor_y,
        },
        outer: layout.outer,
        lines,
    }
}

pub(super) fn clamp_panel_scroll(scroll: usize, body_len: usize, body_rows: usize) -> usize {
    clamp_overlay_scroll(scroll, body_len, body_rows)
}

fn outer_rect(area: Rect, body_line_count: usize) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let horizontal_margin = ((area.width as usize) / 20).clamp(1, 4) as u16;
    let vertical_margin = ((area.height as usize) / 12).clamp(1, 3) as u16;
    let width = area
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2))
        .max(1);
    let max_height = area
        .height
        .saturating_sub(vertical_margin.saturating_mul(2))
        .max(1);
    let desired_height = as_u16(
        body_line_count
            .max(MIN_BODY_ROWS)
            .saturating_add(INNER_CHROME_ROWS)
            .saturating_add(TOP_BORDER_ROWS)
            .saturating_add(BOTTOM_BORDER_ROWS),
    );
    let height = desired_height.min(max_height);
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    Rect::new(x, y, width, height)
}

fn layout_for_outer(outer: Rect) -> OverlayPanelLayout {
    let inner_width = (outer.width as usize).saturating_sub(2).max(1);
    let inner_height = (outer.height as usize).saturating_sub(2).max(1);
    let body_rows = inner_height.saturating_sub(INNER_CHROME_ROWS).max(1);
    OverlayPanelLayout {
        outer,
        inner_width,
        body_rows,
    }
}

fn fit_body_line(line: Line<'static>, width: usize) -> Line<'static> {
    let mut used = 0;
    let mut spans = Vec::new();
    for span in line.spans {
        if used >= width {
            break;
        }
        let span_width = display_width(span.content.as_ref());
        if used + span_width <= width {
            used += span_width;
            spans.push(span);
            continue;
        }
        let truncated = truncate_one_line(span.content.as_ref(), width - used);
        used += display_width(&truncated);
        spans.push(Span::styled(truncated, span.style));
        break;
    }
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    Line::from(spans)
}

fn append_scrollbar_column(rows: &mut [Line<'static>], scrollbar: OverlayScrollbarState) {
    let thumb = scrollbar.thumb();
    for (row, line) in rows.iter_mut().enumerate() {
        line.spans.push(track_span(thumb, row, Theme::accent()));
    }
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

fn horizontal_rule(width: usize) -> Line<'static> {
    if width == 0 {
        return Line::raw("");
    }
    if width == 1 {
        return Line::from(Span::styled("├".to_string(), Theme::dim()));
    }
    let mut text = String::with_capacity(width);
    text.push('├');
    text.push_str(&"─".repeat(width.saturating_sub(2)));
    text.push('┤');
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
    let width = width.max(1);
    let text = truncate_one_line(text, width);
    let pad = width.saturating_sub(display_width(&text));
    Line::from(Span::raw(format!("{text}{}", " ".repeat(pad))))
}

fn as_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

#[cfg(test)]
#[path = "overlay_panel_tests.rs"]
mod tests;
