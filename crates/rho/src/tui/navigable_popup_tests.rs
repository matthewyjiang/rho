use ratatui::{layout::Rect, text::Line};

use super::*;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn sample_items() -> Vec<NavigablePopupItem> {
    vec![
        NavigablePopupItem {
            label: "explorer".into(),
            badge: None,
            selected: true,
        },
        NavigablePopupItem {
            label: "worker".into(),
            badge: None,
            selected: false,
        },
    ]
}

fn long_detail() -> String {
    (0..40)
        .map(|index| format!("detail line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tiny_stacked_layout_keeps_viewports_within_the_body() {
    let layout = navigable_popup_layout(Rect::new(0, 0, 20, 1));

    assert_eq!(layout.orientation, NavigablePopupOrientation::Stacked);
    assert!(layout.detail_viewport_rows + layout.nav_viewport_rows <= layout.body_rows);
    assert_eq!(layout.nav_viewport_rows, 1);
}

#[test]
fn popup_uses_most_of_the_terminal_and_stays_centered() {
    let area = Rect::new(0, 0, 100, 40);
    let outer = navigable_popup_outer_rect(area);

    assert!(outer.width >= 90);
    assert!(outer.height >= 34);
    assert_eq!(outer.x, (area.width - outer.width) / 2);
    assert_eq!(outer.y, (area.height - outer.height) / 2);
}

#[test]
fn side_by_side_layout_keeps_stable_height_and_shows_selected_detail() {
    let layout = navigable_popup_layout(Rect::new(0, 0, 80, 24));
    let detail = navigable_popup_detail_lines(
        "Description\nread-only investigation\n\nTools\nlist_dir, read_file",
        layout.detail_width,
    );
    let items = sample_items();
    let first = navigable_popup_lines(
        layout,
        NavigablePopupContent {
            title: "loaded agents",
            filter: "",
            items: &items,
            selected_position: 0,
            match_count: 2,
            detail: &detail,
            detail_scroll: 0,
            footer: "Enter configure · Esc close",
        },
    );
    let first_text = first.iter().map(line_text).collect::<Vec<_>>().join("\n");

    let mut items = sample_items();
    items[0].selected = false;
    items[1].selected = true;
    let second_detail =
        navigable_popup_detail_lines("Description\nimplementation work", layout.detail_width);
    let second = navigable_popup_lines(
        layout,
        NavigablePopupContent {
            title: "loaded agents",
            filter: "",
            items: &items,
            selected_position: 1,
            match_count: 2,
            detail: &second_detail,
            detail_scroll: 0,
            footer: "Enter configure · Esc close",
        },
    );
    let second_text = second.iter().map(line_text).collect::<Vec<_>>().join("\n");

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

    assert_eq!(layout.orientation, NavigablePopupOrientation::SideBySide);
    assert!(divider_columns
        .iter()
        .all(|column| *column == divider_columns[0]));
    assert_eq!(first.len(), layout.outer.height as usize);
    assert_eq!(second.len(), first.len());
    assert!(first_text.contains('│'));
    assert!(first_text.contains("read-only investigation"));
    assert!(!first_text.contains("implementation work"));
    assert!(second_text.contains("implementation work"));
    assert!(first_text.contains("↑↓"));
    assert!(first_text.contains("PgUp/PgDn"));
}

#[test]
fn stacked_layout_places_detail_above_navigation() {
    let layout = navigable_popup_layout(Rect::new(0, 0, 48, 24));
    let items = sample_items();
    let detail = navigable_popup_detail_lines(
        "Description\nread-only investigation across many files",
        layout.detail_width,
    );
    let lines = navigable_popup_lines(
        layout,
        NavigablePopupContent {
            title: "loaded agents",
            filter: "",
            items: &items,
            selected_position: 0,
            match_count: 2,
            detail: &detail,
            detail_scroll: 0,
            footer: "Enter close · Esc close",
        },
    );
    let text_lines = lines.iter().map(line_text).collect::<Vec<_>>();
    let detail_row = text_lines
        .iter()
        .position(|line| line.contains("read-only investigation"))
        .unwrap();
    let selected_row = text_lines
        .iter()
        .position(|line| line.contains("explorer"))
        .unwrap();

    assert_eq!(layout.orientation, NavigablePopupOrientation::Stacked);
    assert!(!text_lines.iter().any(|line| line.contains(" │ ")));
    assert!(detail_row < selected_row, "{text_lines:#?}");
}

#[test]
fn detail_scroll_reveals_content_below_the_viewport() {
    let layout = navigable_popup_layout(Rect::new(0, 0, 80, 16));
    let detail = navigable_popup_detail_lines(&long_detail(), layout.detail_width);
    let items = sample_items();
    let hidden_index = layout.detail_viewport_rows.saturating_add(5);
    let hidden_marker = format!("detail line {hidden_index:02}");
    let unscroll = navigable_popup_lines(
        layout,
        NavigablePopupContent {
            title: "loaded agents",
            filter: "",
            items: &items,
            selected_position: 0,
            match_count: 2,
            detail: &detail,
            detail_scroll: 0,
            footer: "Enter close · Esc close",
        },
    );
    let scrolled = navigable_popup_lines(
        layout,
        NavigablePopupContent {
            title: "loaded agents",
            filter: "",
            items: &items,
            selected_position: 0,
            match_count: 2,
            detail: &detail,
            detail_scroll: hidden_index,
            footer: "Enter close · Esc close",
        },
    );
    let unscroll_text = unscroll
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let scrolled_text = scrolled
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(unscroll_text.contains("detail line 00"));
    assert!(!unscroll_text.contains(&hidden_marker));
    assert!(scrolled_text.contains(&hidden_marker));
    assert!(!scrolled_text.contains("detail line 00"));
    assert_eq!(unscroll.len(), scrolled.len());
}

#[test]
fn clamp_detail_scroll_respects_viewport() {
    assert_eq!(clamp_detail_scroll(100, 12, 5), 7);
    assert_eq!(clamp_detail_scroll(0, 3, 5), 0);
    assert_eq!(clamp_detail_scroll(2, 10, 10), 0);
}
