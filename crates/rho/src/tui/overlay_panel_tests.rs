use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::{clamp_panel_scroll, overlay_panel_layout, render_overlay_panel};

// Covers: a short body must not fill the terminal, and overflow must clamp scroll.
// Owner: pure unit
#[test]
fn overlay_panel_sizes_to_body_and_clamps_scroll() {
    let area = Rect::new(0, 0, 80, 24);
    let layout = overlay_panel_layout(area, 2);
    assert!(layout.outer.height < area.height);
    assert_eq!(layout.body_rows, 3, "short bodies keep a minimum viewport");
    assert_eq!(clamp_panel_scroll(99, 2, layout.body_rows), 0);

    let long_layout = overlay_panel_layout(area, 40);
    assert_eq!(long_layout.outer.height, area.height.saturating_sub(4));
    let max_scroll = 40usize.saturating_sub(long_layout.body_rows);
    assert_eq!(
        clamp_panel_scroll(max_scroll + 8, 40, long_layout.body_rows),
        max_scroll
    );
}

#[test]
fn overlay_panel_title_is_drawn_on_the_border() {
    let area = Rect::new(0, 0, 60, 20);
    let body = vec![ratatui::text::Line::raw("row")];
    let frame = render_overlay_panel("Usage limits", "esc close", &body, 0, area);
    let title = frame.lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(
        title.contains("Usage limits"),
        "expected titled border, got {title:?}"
    );
    assert!(
        !title.contains("Search"),
        "document overlay must not draw picker search chrome"
    );
}

#[test]
fn overlay_panel_clips_body_to_inner_width_when_scrollbar_is_shown() {
    let area = Rect::new(0, 0, 40, 12);
    let body = (0..20)
        .map(|i| ratatui::text::Line::raw(format!("row-{i:02} {}", "x".repeat(80))))
        .collect::<Vec<_>>();
    let frame = render_overlay_panel("Title", "esc close", &body, 0, area);
    let widths: Vec<usize> = frame
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| super::display_width(span.content.as_ref()))
                .sum()
        })
        .collect();
    assert!(
        widths
            .iter()
            .all(|width| *width <= frame.outer.width as usize),
        "overlay rows must not overflow the panel, got {widths:?}"
    );
}
