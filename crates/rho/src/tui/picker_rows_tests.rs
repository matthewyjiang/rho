use super::*;
use crate::tui::{PickerBadge, PickerBadgeTone, PickerItem};
use pretty_assertions::assert_eq;

fn item(label: &str, section: Option<&str>) -> PickerItem {
    PickerItem {
        label: label.into(),
        section: section.map(Into::into),
        detail: None,
        preview: None,
        badge: None,
        value: label.to_ascii_lowercase(),
        selection_verb: None,
    }
}

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn aligned_layout(width: usize, column: usize) -> RowLayout {
    RowLayout {
        width,
        width_mode: RowWidthMode::AlignedColumn(column),
        show_badges: true,
        show_preview: true,
        fill: crate::tui::LineFill::Natural,
    }
}

// Covers: list-layout pickers must render section headers on group transitions;
// before unification the inline list dropped sections entirely.
// Owner: pure unit (shared picker row generation)
#[test]
fn aligned_rows_insert_section_headers_on_transitions() {
    let items = vec![
        item("alpha", Some("FIRST")),
        item("beta", Some("FIRST")),
        item("gamma", Some("SECOND")),
    ];
    let matching = vec![0, 1, 2];
    let rows = picker_item_rows(&items, &matching, 2, aligned_layout(40, 12), None);

    let texts = rows
        .rows
        .iter()
        .map(|line| line_text(line).trim_end().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        vec!["  FIRST", "  alpha", "  beta", "  SECOND", "→ gamma"]
    );
    assert_eq!(rows.selected_row, 4);
}

// Covers: header rows shift the selected row index; wrong accounting scrolls
// the selection off screen.
// Owner: pure unit (shared picker row generation)
#[test]
fn selected_row_index_ignores_headers_before_it() {
    let items = vec![item("one", None), item("two", Some("GROUP"))];
    let rows = picker_item_rows(&items, &[0, 1], 1, aligned_layout(30, 12), None);
    // one, GROUP header, two → selected "two" sits at row 2.
    assert_eq!(rows.selected_row, 2);
    assert_eq!(rows.rows.len(), 3);
}

// Covers: the shared scroll window must keep the selected row visible and
// tolerate zero-row viewports.
// Owner: pure unit (scroll window math)
#[test]
fn scroll_window_start_keeps_selected_visible() {
    assert_eq!(scroll_window_start(0, 5), 0);
    assert_eq!(scroll_window_start(4, 5), 0);
    assert_eq!(scroll_window_start(5, 5), 1);
    assert_eq!(scroll_window_start(9, 5), 5);
    assert_eq!(scroll_window_start(3, 0), 3);
}

// Covers: pane-filling rows must pad to the exact pane width with the badge
// kept visible, or the overlay column rule drifts.
// Owner: pure unit (shared picker row generation)
#[test]
fn fill_pane_rows_pad_label_and_keep_badge() {
    let mut badged = item("workhorse", None);
    badged.badge = Some(PickerBadge {
        text: "editable".into(),
        tone: PickerBadgeTone::Editable,
    });
    let rows = picker_item_rows(
        &[badged],
        &[0],
        0,
        RowLayout {
            width: 24,
            width_mode: RowWidthMode::FillPane,
            show_badges: true,
            show_preview: false,
            fill: crate::tui::LineFill::PadToWidth,
        },
        None,
    );
    let text = line_text(&rows.rows[0]);
    assert_eq!(text, "→ workhorse     editable");
    assert_eq!(text.chars().count(), 24);
}

// Covers: aligned rows must order label column, badge, then preview within
// the row budget.
// Owner: pure unit (shared picker row generation)
#[test]
fn aligned_rows_show_preview_after_badge() {
    let mut full = item("model", None);
    full.badge = Some(PickerBadge {
        text: "pinned".into(),
        tone: PickerBadgeTone::Favorite,
    });
    full.preview = Some("fast default".into());
    let rows = picker_item_rows(&[full], &[0], 0, aligned_layout(60, 12), None);
    assert_eq!(
        line_text(&rows.rows[0]),
        "→ model         pinned  fast default"
    );
}

// Covers: the label column follows the widest label inside fixed caps so
// columns stay stable while scrolling.
// Owner: pure unit (label column policy)
#[test]
fn label_column_width_tracks_widest_label_within_caps() {
    let narrow = vec![item("ab", None)];
    assert_eq!(label_column_width(&narrow, 80), 12);

    let wide = vec![item(&"x".repeat(90), None)];
    assert_eq!(label_column_width(&wide, 200), 60);

    let medium = vec![item(&"y".repeat(30), None)];
    assert_eq!(label_column_width(&medium, 80), 30);
}
