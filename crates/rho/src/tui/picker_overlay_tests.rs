use pretty_assertions::assert_eq;
use ratatui::{layout::Rect, text::Line};

use super::super::{
    PickerAction, PickerBadge, PickerBadgePlacement, PickerBadgeTone, PickerItem, PickerLayout,
    UiPicker,
};
use super::*;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn sample_picker(detail_a: &str, detail_b: &str) -> UiPicker {
    UiPicker::new(
        "loaded agents",
        "help",
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
            },
            PickerItem {
                section: None,
                label: "worker".into(),
                detail: Some(detail_b.into()),
                preview: None,
                badge: None,
                value: "worker".into(),
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

fn tree_like_picker() -> UiPicker {
    UiPicker::new(
        "Conversation tree",
        "help",
        vec![
            PickerItem {
                section: None,
                label: "◆ root turn".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "root".into(),
            },
            PickerItem {
                section: None,
                label: "└─ ◆ branch turn".into(),
                detail: None,
                preview: None,
                badge: Some(PickerBadge {
                    text: "active".into(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: "branch".into(),
            },
        ],
        PickerAction::SelectTreeNode,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " TREE".into(),
        detail_label: None,
        nav_keys_hint: "↑↓ turns".into(),
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

    let frame = render_picker_overlay(&picker, Rect::new(0, 0, 100, 30));
    let rendered = frame.lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("CUSTOM")));
    assert!(rendered.iter().all(|line| !line.contains("INTERNAL")));
    assert_eq!(picker.selected_item().unwrap().label, "worker");
}

#[test]
fn detail_badges_move_status_out_of_navigation_rows() {
    let picker = sample_picker("agent detail", "worker detail")
        .with_badge_placement(PickerBadgePlacement::Detail);

    let frame = render_picker_overlay(&picker, Rect::new(0, 0, 100, 30));
    let selected_row = frame
        .lines
        .iter()
        .map(line_text)
        .find(|line| line.contains("→ explorer"))
        .unwrap();
    let (navigation, detail) = selected_row.split_once(SEPARATOR).unwrap();

    assert!(!navigation.contains("internal"), "{selected_row}");
    assert!(detail.contains("Status  internal"), "{selected_row}");
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

    for width in [12_u16, 18, 24, 36, 48] {
        let frame = render_picker_overlay(&long_badge, Rect::new(0, 0, width, 20));
        for line in &frame.lines {
            let text = line_text(line);
            let measured = super::super::display_width(&text);
            assert!(
                measured <= width as usize,
                "width {width}: measured {measured} for {text:?}"
            );
        }
        assert!(
            frame
                .lines
                .iter()
                .map(line_text)
                .any(|line| line.contains("Status")
                    || line.contains("…")
                    || line.contains("healthy")),
            "expected a detail badge row at width {width}"
        );
    }
}

#[test]
fn tiny_stacked_layout_keeps_viewports_within_the_body() {
    let layout = picker_overlay_layout(Rect::new(0, 0, 20, 1), /*has_details*/ true);
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
    assert!(detail_viewport_rows + nav_viewport_rows <= layout.body_rows);
    assert_eq!(nav_viewport_rows, 1);
}

#[test]
fn popup_uses_most_of_the_terminal_and_stays_centered() {
    let area = Rect::new(0, 0, 100, 40);
    let layout = picker_overlay_layout(area, /*has_details*/ true);
    let outer = layout.outer;

    assert!(outer.width >= 90);
    assert!(outer.height >= 34);
    assert_eq!(outer.x, (area.width - outer.width) / 2);
    assert_eq!(outer.y, (area.height - outer.height) / 2);
}

#[test]
fn side_by_side_layout_keeps_stable_height_and_shows_selected_detail() {
    let area = Rect::new(0, 0, 80, 24);
    let layout = picker_overlay_layout(area, /*has_details*/ true);
    let mut picker = sample_picker(
        "Description\nread-only investigation\n\nTools\nlist_dir, read_file",
        "Description\nimplementation work",
    );
    let first = render_picker_overlay(&picker, area);
    let first_text = first
        .lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    picker.selected = 1;
    picker.reset_detail_scroll();
    let second = render_picker_overlay(&picker, area);
    let second_text = second
        .lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    let OverlayPanes::NavAndDetail { orientation, .. } = nav_and_detail_panes(&layout) else {
        unreachable!()
    };
    let divider_columns = first_text
        .lines()
        .skip(3)
        .take(layout.body_rows)
        .map(|line| {
            let divider = line
                .find(" │ ")
                .expect("each body row should keep its divider");
            line[..divider].chars().count()
        })
        .collect::<Vec<_>>();

    assert_eq!(orientation, OverlayOrientation::SideBySide);
    assert!(divider_columns
        .iter()
        .all(|column| *column == divider_columns[0]));
    assert_eq!(first.lines.len(), layout.outer.height as usize);
    assert_eq!(second.lines.len(), first.lines.len());
    assert!(first_text.contains('│'));
    assert!(first_text.contains("read-only investigation"));
    assert!(first_text.contains("internal"));
    assert!(!first_text.contains("implementation work"));
    assert!(second_text.contains("implementation work"));
    assert!(first_text.contains("↑↓"));
    assert!(first_text.contains("PgUp/PgDn"));
    assert!(first_text.contains("AGENTS"));
    assert!(first_text.contains("DETAILS"));
    assert_eq!(
        first.cursor,
        ratatui::layout::Position {
            x: layout
                .outer
                .x
                .saturating_add(1)
                .saturating_add(filter_cursor_x("", layout.inner_width)),
            y: layout.outer.y.saturating_add(1),
        }
    );
}

#[test]
fn stacked_layout_places_detail_above_navigation() {
    let area = Rect::new(0, 0, 48, 24);
    let layout = picker_overlay_layout(area, /*has_details*/ true);
    let picker = sample_picker(
        "Description\nread-only investigation across many files",
        "Description\nimplementation work",
    );
    let frame = render_picker_overlay(&picker, area);
    let text_lines = frame.lines.iter().map(line_text).collect::<Vec<_>>();
    let detail_row = text_lines
        .iter()
        .position(|line| line.contains("read-only investigation"))
        .unwrap();
    let selected_row = text_lines
        .iter()
        .position(|line| line.contains("explorer"))
        .unwrap();

    let OverlayPanes::NavAndDetail { orientation, .. } = nav_and_detail_panes(&layout) else {
        unreachable!()
    };
    assert_eq!(orientation, OverlayOrientation::Stacked);
    assert!(!text_lines.iter().any(|line| line.contains(" │ ")));
    assert!(detail_row < selected_row, "{text_lines:#?}");
}

#[test]
fn missing_item_details_uses_full_width_navigation() {
    let area = Rect::new(0, 0, 80, 24);
    let layout = picker_overlay_layout(area, /*has_details*/ false);
    let picker = tree_like_picker();
    let frame = render_picker_overlay(&picker, area);
    let text = frame
        .lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!picker.has_item_details());
    assert!(!picker.has_scrollable_detail());
    assert_eq!(
        layout.panes,
        OverlayPanes::NavOnly {
            nav_width: layout.inner_width,
            nav_viewport_rows: layout.body_rows,
        }
    );
    assert_eq!(
        layout.page_target(),
        OverlayPageTarget::Nav {
            rows: layout.body_rows.max(1)
        }
    );
    assert!(text.contains(" TREE"));
    assert!(text.contains("root turn"));
    assert!(text.contains("branch turn"));
    assert!(!text.contains(" DETAILS"));
    assert!(!text.contains(" │ "));
    assert!(!text.contains("PgUp/PgDn details"));
    assert!(text.contains("PgUp/PgDn"));
    assert!(text.contains("↑↓ turns"));
}

#[test]
fn detail_scroll_reveals_content_below_the_viewport() {
    let area = Rect::new(0, 0, 80, 16);
    let layout = picker_overlay_layout(area, /*has_details*/ true);
    let mut picker = sample_picker(&long_detail(), "other");
    let viewport = layout.detail_viewport().expect("detail viewport");
    let hidden_index = viewport.rows.saturating_add(5);
    let hidden_marker = format!("detail line {hidden_index:02}");
    let unscroll = render_picker_overlay(&picker, area);
    picker.scroll_detail_by(hidden_index as isize, viewport);
    let scrolled = render_picker_overlay(&picker, area);
    let unscroll_text = unscroll
        .lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let scrolled_text = scrolled
        .lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(unscroll_text.contains("detail line 00"));
    assert!(!unscroll_text.contains(&hidden_marker));
    assert!(scrolled_text.contains(&hidden_marker));
    assert!(!scrolled_text.contains("detail line 00"));
    assert_eq!(unscroll.lines.len(), scrolled.lines.len());
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
    let layout = picker_overlay_layout(area, /*has_details*/ true);
    let mut picker = sample_picker(&long_detail(), "other");
    let viewport = layout.detail_viewport().expect("detail viewport");
    picker.scroll_detail_end(viewport);
    let line_count = overlay_detail_lines(picker.selected_detail(), viewport.width).len();
    let expected = line_count.saturating_sub(viewport.rows.max(1));
    assert_eq!(picker.detail_scroll, expected);
}
