use pretty_assertions::assert_eq;
use ratatui::{layout::Rect, text::Line};

use super::super::{
    PickerAction, PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
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
        detail_label: " DETAILS".into(),
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
fn tiny_stacked_layout_keeps_viewports_within_the_body() {
    let layout = picker_overlay_layout(Rect::new(0, 0, 20, 1));

    assert_eq!(layout.orientation, OverlayOrientation::Stacked);
    assert!(layout.detail_viewport_rows + layout.nav_viewport_rows <= layout.body_rows);
    assert_eq!(layout.nav_viewport_rows, 1);
}

#[test]
fn popup_uses_most_of_the_terminal_and_stays_centered() {
    let area = Rect::new(0, 0, 100, 40);
    let layout = picker_overlay_layout(area);
    let outer = layout.outer;

    assert!(outer.width >= 90);
    assert!(outer.height >= 34);
    assert_eq!(outer.x, (area.width - outer.width) / 2);
    assert_eq!(outer.y, (area.height - outer.height) / 2);
}

#[test]
fn side_by_side_layout_keeps_stable_height_and_shows_selected_detail() {
    let area = Rect::new(0, 0, 80, 24);
    let layout = picker_overlay_layout(area);
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

    assert_eq!(layout.orientation, OverlayOrientation::SideBySide);
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
    let layout = picker_overlay_layout(area);
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

    assert_eq!(layout.orientation, OverlayOrientation::Stacked);
    assert!(!text_lines.iter().any(|line| line.contains(" │ ")));
    assert!(detail_row < selected_row, "{text_lines:#?}");
}

#[test]
fn detail_scroll_reveals_content_below_the_viewport() {
    let area = Rect::new(0, 0, 80, 16);
    let layout = picker_overlay_layout(area);
    let mut picker = sample_picker(&long_detail(), "other");
    let hidden_index = layout.detail_viewport_rows.saturating_add(5);
    let hidden_marker = format!("detail line {hidden_index:02}");
    let unscroll = render_picker_overlay(&picker, area);
    picker.scroll_detail_by(hidden_index as isize, layout.detail_viewport());
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
    let layout = picker_overlay_layout(area);
    let mut picker = sample_picker(&long_detail(), "other");
    picker.scroll_detail_end(layout.detail_viewport());
    let line_count = overlay_detail_lines(picker.selected_detail(), layout.detail_width).len();
    let expected = line_count.saturating_sub(layout.detail_viewport_rows.max(1));
    assert_eq!(picker.detail_scroll_top(), expected);
    assert_ne!(picker.detail_scroll_top(), usize::MAX);
}
