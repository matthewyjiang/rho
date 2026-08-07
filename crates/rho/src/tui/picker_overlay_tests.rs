use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::super::{
    PickerAction, PickerBadge, PickerBadgePlacement, PickerBadgeTone, PickerItem, PickerLayout,
    UiPicker,
};
use super::*;

fn sample_picker(detail_a: &str, detail_b: &str) -> UiPicker {
    UiPicker::new(
        "loaded agents",
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
            },
            PickerItem {
                section: None,
                label: "worker".into(),
                detail: Some(detail_b.into()),
                preview: None,
                badge: None,
                value: "worker".into(),
                selection_verb: None,
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

fn nav_and_detail_panes(layout: &OverlayLayout) -> OverlayPanes {
    match layout.panes {
        panes @ OverlayPanes::NavAndDetail { .. } => panes,
        OverlayPanes::NavOnly { .. } => panic!("expected nav+detail panes, got nav-only"),
    }
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
            let text_width = super::super::display_width(
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
fn tiny_stacked_layout_keeps_viewports_within_the_body() {
    let layout = picker_overlay_layout(
        Rect::new(0, 0, 20, 1),
        OverlaySizing {
            has_details: true,
            nav_rows: 2,
        },
    );
    let OverlayPanes::NavAndDetail {
        orientation,
        detail_viewport_rows,
        nav_viewport_rows,
        ..
    } = nav_and_detail_panes(&layout)
    else {
        unreachable!()
    };

    assert_eq!(orientation, OverlayOrientation::Stacked);
    let separator_rows = usize::from(layout.body_rows > 2);
    assert!(detail_viewport_rows + nav_viewport_rows + separator_rows <= layout.body_rows);
    assert_eq!(nav_viewport_rows, 1);
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

// Covers: the overlay height must follow the item count instead of always
// filling the screen; short pickers get a compact box, long pickers still
// clamp to the margin-bounded maximum.
// Owner: tui picker_overlay geometry
#[test]
fn overlay_height_follows_item_count() {
    let area = Rect::new(0, 0, 120, 40);
    let small = picker_overlay_layout(
        area,
        OverlaySizing {
            has_details: true,
            nav_rows: 5,
        },
    );
    // 12-row detail minimum + 5 chrome rows + 2 border rows.
    assert_eq!(small.outer.height, 19);

    let nav_only = picker_overlay_layout(
        area,
        OverlaySizing {
            has_details: false,
            nav_rows: 5,
        },
    );
    // 5 nav rows + 5 chrome rows + 2 border rows.
    assert_eq!(nav_only.outer.height, 12);

    let large = picker_overlay_layout(
        area,
        OverlaySizing {
            has_details: true,
            nav_rows: 100,
        },
    );
    // Clamped to the screen minus vertical margins.
    assert_eq!(large.outer.height, 34);
    assert!(large.outer.y > area.y);
}
