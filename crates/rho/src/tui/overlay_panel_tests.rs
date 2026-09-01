use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::{
    clamp_panel_scroll, classify_panel_key, overlay_panel_inner_width, overlay_panel_layout,
    render_overlay_panel, PanelKey, PanelScroll, PanelScrollTarget,
};

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
    assert_eq!(overlay_panel_inner_width(area), layout.inner_width);
    assert_eq!(overlay_panel_inner_width(area), long_layout.inner_width);
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
    let frame = render_overlay_panel("Usage limits", "Enter/Esc close", &body, 0, area);
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
    let frame = render_overlay_panel("Title", "Enter/Esc close", &body, 0, area);
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

// Covers: dismiss-only overlays share one key table so a third overlay cannot
// drift from Enter/Esc/q close, hjkl/arrows/paging, Ctrl+C passthrough, and
// swallow-everything-else.
// Owner: pure unit
#[test]
fn classify_panel_key_covers_close_scroll_passthrough_and_swallow() {
    let cases = [
        (KeyCode::Esc, KeyModifiers::NONE, PanelKey::Close),
        (KeyCode::Enter, KeyModifiers::NONE, PanelKey::Close),
        (KeyCode::Char('q'), KeyModifiers::NONE, PanelKey::Close),
        (
            KeyCode::Up,
            KeyModifiers::NONE,
            PanelKey::Scroll(PanelScrollTarget::Delta(-1)),
        ),
        (
            KeyCode::Char('k'),
            KeyModifiers::NONE,
            PanelKey::Scroll(PanelScrollTarget::Delta(-1)),
        ),
        (
            KeyCode::Down,
            KeyModifiers::NONE,
            PanelKey::Scroll(PanelScrollTarget::Delta(1)),
        ),
        (
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            PanelKey::Scroll(PanelScrollTarget::Delta(1)),
        ),
        (
            KeyCode::PageUp,
            KeyModifiers::NONE,
            PanelKey::Scroll(PanelScrollTarget::Page(-1)),
        ),
        (
            KeyCode::PageDown,
            KeyModifiers::SHIFT,
            PanelKey::Scroll(PanelScrollTarget::Page(1)),
        ),
        (
            KeyCode::Home,
            KeyModifiers::NONE,
            PanelKey::Scroll(PanelScrollTarget::Absolute(0)),
        ),
        (
            KeyCode::End,
            KeyModifiers::NONE,
            PanelKey::Scroll(PanelScrollTarget::Absolute(usize::MAX)),
        ),
        (
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            PanelKey::Passthrough,
        ),
        (KeyCode::Char('x'), KeyModifiers::NONE, PanelKey::Swallow),
    ];
    for (code, modifiers, expected) in cases {
        assert_eq!(
            classify_panel_key(KeyEvent::new(code, modifiers)),
            expected,
            "{code:?} {modifiers:?}"
        );
    }
}

// Covers: delta, page, and absolute targets clamp to the body, so overlays
// cannot scroll past the last row.
// Owner: pure unit
#[test]
fn panel_scroll_applies_targets_and_clamps() {
    let mut scroll = PanelScroll::default();
    scroll.apply(PanelScrollTarget::Delta(2), 10, 4);
    assert_eq!(scroll.offset(), 2);
    scroll.apply(PanelScrollTarget::Page(1), 10, 4);
    assert_eq!(scroll.offset(), 6);
    scroll.apply(PanelScrollTarget::Absolute(99), 10, 4);
    assert_eq!(scroll.offset(), 6);
    scroll.apply(PanelScrollTarget::Delta(-3), 10, 4);
    assert_eq!(scroll.offset(), 3);
}
