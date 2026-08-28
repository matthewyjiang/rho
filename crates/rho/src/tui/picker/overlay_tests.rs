use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::super::{
    PickerAction, PickerBadge, PickerBadgePlacement, PickerBadgeTone, PickerItem, PickerLayout,
    UiPicker,
};
use super::*;

fn sample_picker(detail_a: &str, detail_b: &str) -> UiPicker {
    UiPicker::new(
        "Loaded agents",
        vec![
            PickerItem {
                section: None,
                label: "explorer".into(),
                detail: Some(detail_a.into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: "internal".into(),
                    tone: PickerBadgeTone::Internal,
                }),
                value: "explorer".into(),
                selection_verb: None,
                allow_filter_completion: true,
            },
            PickerItem {
                section: None,
                label: "worker".into(),
                detail: Some(detail_b.into()),
                preview: None,
                badge: None,
                value: "worker".into(),
                selection_verb: None,
                allow_filter_completion: true,
            },
        ],
        PickerAction::ViewAgent,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " AGENTS".into(),
        detail_label: Some(" DETAILS".into()),
        nav_keys_hint: "↑↓ agents".into(),
    })
}

fn long_detail() -> String {
    (0..40)
        .map(|index| format!("detail line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn section_headers_follow_filtered_items_without_becoming_selectable() {
    let mut picker = sample_picker("agent detail", "worker detail");
    picker.items[0].section = Some("INTERNAL".into());
    picker.items[1].section = Some("CUSTOM".into());
    picker.filter = "custom".into();
    picker.select_first_match();

    assert_eq!(picker.matching_indices(), vec![1]);
    assert_eq!(picker.selected_item().unwrap().label, "worker");
    assert_eq!(
        picker.selected_item().unwrap().section.as_deref(),
        Some("CUSTOM")
    );
}

#[test]
fn detail_badge_rows_never_exceed_narrow_overlay_widths() {
    let picker = sample_picker("agent detail", "worker detail")
        .with_badge_placement(PickerBadgePlacement::Detail);
    let mut long_badge = picker;
    long_badge.items[0].badge = Some(PickerBadge {
        text: "healthy-and-also-very-long-status-label".into(),
        tone: PickerBadgeTone::Healthy,
    });

    for width in [8_u16, 12, 18, 24, 36, 48] {
        let frame = render_picker_overlay(&long_badge, Rect::new(0, 0, width, 20));
        for line in &frame.lines {
            let text_width = crate::tui::render::display_width(
                &line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>(),
            );
            assert!(
                text_width <= width as usize,
                "width {width}: overflow text_width {text_width}"
            );
        }
    }
}

#[test]
fn clamp_detail_scroll_respects_viewport() {
    assert_eq!(clamp_detail_scroll(100, 12, 5), 7);
    assert_eq!(clamp_detail_scroll(0, 3, 5), 0);
    assert_eq!(clamp_detail_scroll(2, 10, 10), 0);
}

#[test]
fn overlay_detail_end_scroll_uses_max_without_sentinel() {
    let area = Rect::new(0, 0, 80, 16);
    let mut picker = sample_picker(&long_detail(), "other");
    let layout = picker_overlay_layout(area, picker.overlay_sizing());
    let viewport = layout.detail_viewport().expect("detail viewport");
    picker.scroll_detail_end(viewport);
    let line_count = overlay_detail_lines(picker.selected_detail(), viewport.width).len();
    let expected = line_count.saturating_sub(viewport.rows.max(1));
    assert_eq!(picker.detail_scroll, expected);
}

// Covers: empty overlay panes must show a no-match / invalid-regex cue instead
// of a blank body that looks identical to a render bug.
// Owner: tui picker_overlay empty state
#[test]
fn overlay_empty_match_state_is_visible() {
    let mut picker = sample_picker("agent detail", "worker detail");
    picker.filter = "zzzz-no-match".into();
    picker.select_first_match();
    let expected = picker.empty_match_message();
    let frame = render_picker_overlay(&picker, Rect::new(0, 0, 80, 20));
    let text = frame
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains(expected),
        "expected empty-state label {expected:?} in overlay body: {text:?}"
    );

    picker.filter = "(".into();
    picker.select_first_match();
    let expected = picker.empty_match_message();
    let frame = render_picker_overlay(&picker, Rect::new(0, 0, 80, 20));
    let text = frame
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains(expected),
        "expected empty-state label {expected:?} in overlay body: {text:?}"
    );
}

// Covers: overflowing panes must render a scrollbar so overflow is visible;
// panes that fit stay bar-free.
// Owner: tui picker_overlay geometry
#[test]
fn overflowing_panes_render_scrollbars() {
    let items = (0..50)
        .map(|index| PickerItem {
            section: None,
            label: format!("agent-{index:02}"),
            detail: Some(long_detail()),
            preview: None,
            badge: None,
            value: format!("agent-{index:02}"),
            selection_verb: None,
            allow_filter_completion: true,
        })
        .collect();
    let picker =
        UiPicker::new("agents", items, PickerAction::ViewAgent).with_layout(PickerLayout::Overlay);
    let frame = render_picker_overlay(&picker, Rect::new(0, 0, 100, 24));
    let body_line = frame
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .find(|text| text.contains('█'))
        .expect("expected a scrollbar thumb in an overflowing overlay");
    // Both panes overflow, so the thumb row carries one thumb per pane.
    assert_eq!(body_line.matches('█').count(), 2, "nav and detail thumbs");

    let short = sample_picker("fits", "also fits");
    let frame = render_picker_overlay(&short, Rect::new(0, 0, 100, 24));
    let text = frame
        .lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
        .collect::<String>();
    assert!(
        !text.contains('█'),
        "fitting panes must not render a scrollbar"
    );
}
