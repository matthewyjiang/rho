//! The inline (non-overlay) list picker: filter line, sectioned item rows in
//! a scroll window, position count, selected detail, and key-hint footer.

use ratatui::text::{Line, Span};

use super::{styled_line, truncate_one_line, LineFill};
use crate::tui::{
    composer_chrome::wrap_footer_parts,
    composer_pointer::{ComposerHit, PickerRowTarget},
    picker::{label_column_width, picker_item_rows, PickerDetail, RowLayout, RowWidthMode},
    theme::Theme,
    UiPicker,
};

/// Rows outside the picker that must stay visible: history headroom, the
/// composer divider, and the statusline.
const PICKER_RESERVED_FEED_ROWS: usize = 5;

/// Rows the inline list picker spends on its own chrome, matching what
/// `list_picker_frame` emits around the item rows.
fn list_picker_chrome_rows(picker: &UiPicker, footer_rows: usize) -> usize {
    // filter + blank + count + blank + footer lines, plus detail + blank when shown.
    4 + footer_rows + if picker.has_item_details() { 2 } else { 0 }
}

/// Item rows a picker can list in a `viewport_height` row terminal.
///
/// The list grows with the terminal instead of staying at the number that fits
/// the default height fallback, so a tall window shows a long model or session
/// list without scrolling.
fn picker_visible_item_cap(picker: &UiPicker, viewport_height: usize, footer_rows: usize) -> usize {
    viewport_height
        .saturating_sub(list_picker_chrome_rows(picker, footer_rows))
        .saturating_sub(PICKER_RESERVED_FEED_ROWS)
        .max(1)
}

/// Inline list picker rows plus one pointer hit per painted item row.
///
/// Hit line indices count from the first picker line, and the selected row's
/// hit is marked active. Section headers, the filter, and the footer take no
/// hits.
pub(in crate::tui) struct ListPickerFrame {
    pub(in crate::tui) lines: Vec<Line<'static>>,
    pub(in crate::tui) hits: Vec<ComposerHit<PickerRowTarget>>,
}

/// Renders the inline list picker. Hover is painted on top from the hits.
///
/// The item window comes from [`UiPicker::nav_window_start`], so it only
/// scrolls when the selection leaves it, and a click can hold it in place.
pub(in crate::tui) fn list_picker_frame(
    picker: &UiPicker,
    width: usize,
    viewport_height: usize,
) -> ListPickerFrame {
    let footer_text = list_picker_footer_text(picker, width);
    let footer_lines = list_picker_footer_lines(&footer_text, width);
    let item_cap = picker_visible_item_cap(picker, viewport_height, footer_text.len());
    let matching_indices = picker.matching_indices();
    let mut lines = Vec::with_capacity(item_cap + 7);
    let mut hits = Vec::new();
    lines.push(picker_filter_line(picker, width));
    lines.push(Line::raw(""));

    if matching_indices.is_empty() {
        lines.push(styled_line(
            truncate_one_line(&format!("  {}", picker.empty_match_message()), width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
        lines.push(Line::raw(""));
        lines.extend(footer_lines);
        return ListPickerFrame { lines, hits };
    }

    let row_layout = RowLayout {
        width,
        width_mode: RowWidthMode::AlignedColumn(label_column_width(&picker.items, width)),
        show_badges: true,
        show_preview: true,
        fill: LineFill::Natural,
    };
    let rows = picker_item_rows(
        &picker.items,
        &matching_indices,
        picker.selected,
        row_layout,
        /*hovered_row*/ None,
    );
    let start = picker.nav_window_start(item_cap);
    for (line, item) in rows
        .rows
        .into_iter()
        .zip(rows.row_items)
        .skip(start)
        .take(item_cap)
    {
        if let Some(item) = item {
            hits.push(
                ComposerHit::rows(
                    lines.len()..lines.len() + 1,
                    PickerRowTarget {
                        item,
                        window_start: start,
                    },
                )
                .with_active(item == picker.selected),
            );
        }
        lines.push(line);
    }

    let selected_position = matching_indices
        .iter()
        .position(|index| *index == picker.selected)
        .unwrap_or(0);
    lines.push(styled_line(
        truncate_one_line(
            &format!("  ({}/{})", selected_position + 1, matching_indices.len()),
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    lines.push(Line::raw(""));
    if picker.has_item_details() {
        let detail = picker
            .selected_detail()
            .map(PickerDetail::plain_text)
            .unwrap_or_default();
        let detail = truncate_one_line(&detail, width.saturating_sub(2));
        let detail = if width > 2 {
            format!("  {detail}")
        } else {
            truncate_one_line(&detail, width)
        };
        lines.push(styled_line(detail, width, Theme::dim(), LineFill::Natural));
        lines.push(Line::raw(""));
    }
    lines.extend(footer_lines);
    ListPickerFrame { lines, hits }
}

fn list_picker_footer_text(picker: &UiPicker, width: usize) -> Vec<String> {
    let parts = picker.list_footer_parts();
    let inner_width = width.saturating_sub(2);
    let indent = width > 2;
    wrap_footer_parts(parts.iter().map(String::as_str), inner_width)
        .into_iter()
        .map(|line| if indent { format!("  {line}") } else { line })
        .collect()
}

fn list_picker_footer_lines(footer_text: &[String], width: usize) -> Vec<Line<'static>> {
    footer_text
        .iter()
        .map(|line| {
            styled_line(
                truncate_one_line(line, width),
                width,
                Theme::dim(),
                LineFill::Natural,
            )
        })
        .collect()
}

fn picker_filter_line(picker: &UiPicker, width: usize) -> Line<'static> {
    if width <= 1 {
        return Line::from(Span::styled(">", Theme::text_strong()));
    }

    Line::from(vec![
        Span::styled(">", Theme::text_strong()),
        Span::raw(" "),
        Span::styled(
            truncate_one_line(&picker.filter, width.saturating_sub(2)),
            Theme::text_strong(),
        ),
    ])
}
