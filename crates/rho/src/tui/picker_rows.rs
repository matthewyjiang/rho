//! Shared picker row mechanics: section headers, item rows, badge styling,
//! and the scroll window that keeps the selected row visible.
//!
//! Both the overlay nav pane and the inline list picker build their rows here,
//! so grouping, badges, and scrolling cannot drift between the two layouts.
//! Feature policy (what items mean, which badges exist) stays at call sites.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{
    display_width, styled_line, truncate_one_line, LineFill, PickerBadgeTone, PickerItem, Theme,
};

/// Widest label column an aligned list row reserves before badge and preview.
const MAX_LABEL_COLUMN: usize = 60;
const MIN_LABEL_COLUMN: usize = 12;
/// Longest badge text a pane-filling nav row shows before truncation.
const NAV_BADGE_CAP: usize = 16;

pub(super) fn picker_badge_style(tone: PickerBadgeTone) -> Style {
    match tone {
        PickerBadgeTone::Internal | PickerBadgeTone::Editable => Theme::accent(),
        PickerBadgeTone::Selected => Theme::warning(),
        PickerBadgeTone::Favorite | PickerBadgeTone::Healthy => Theme::success(),
        PickerBadgeTone::Warning => Theme::warning(),
    }
}

/// How an item row uses its horizontal space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowWidthMode {
    /// The label pads to the pane width (minus badge) so the selection reads
    /// as a block. Used by the overlay nav pane.
    FillPane,
    /// The label aligns to a shared column; badge and preview follow. Used by
    /// the inline list.
    AlignedColumn(usize),
}

/// Presentation choices for one batch of picker rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RowLayout {
    pub(super) width: usize,
    pub(super) width_mode: RowWidthMode,
    pub(super) show_badges: bool,
    pub(super) show_preview: bool,
    pub(super) fill: LineFill,
}

/// Item and section-header rows plus the row index of the selected item.
pub(super) struct PickerRows {
    pub(super) rows: Vec<Line<'static>>,
    pub(super) selected_row: usize,
}

/// Shared label column width for aligned list rows.
///
/// The widest label is taken across every item, not just the visible window,
/// so the column does not jump while scrolling.
pub(super) fn label_column_width(items: &[PickerItem], width: usize) -> usize {
    let reserved_preview_width = width.saturating_sub(18);
    let available_width = if reserved_preview_width >= MIN_LABEL_COLUMN {
        reserved_preview_width
    } else {
        width.saturating_sub(2).max(1)
    };
    let max_label_width = MAX_LABEL_COLUMN.min(available_width);
    let min_label_width = MIN_LABEL_COLUMN.min(max_label_width).max(1);
    items
        .iter()
        .map(|item| display_width(&item.label))
        .max()
        .unwrap_or(min_label_width)
        .clamp(min_label_width, max_label_width)
}

/// Rows [`picker_item_rows`] will emit: matching items plus section headers.
pub(super) fn picker_row_count(items: &[PickerItem], matching: &[usize]) -> usize {
    let mut count = 0;
    let mut current_section: Option<&str> = None;
    for index in matching.iter().copied() {
        let Some(item) = items.get(index) else {
            continue;
        };
        if item.section.as_deref() != current_section {
            current_section = item.section.as_deref();
            count += usize::from(current_section.is_some());
        }
        count += 1;
    }
    count
}

/// Row-space index of the selected item among the matching rows, counting
/// section headers.
pub(super) fn selected_row_index(
    items: &[PickerItem],
    matching: &[usize],
    selected: usize,
) -> usize {
    let mut row = 0;
    let mut current_section: Option<&str> = None;
    for index in matching.iter().copied() {
        let Some(item) = items.get(index) else {
            continue;
        };
        if item.section.as_deref() != current_section {
            current_section = item.section.as_deref();
            row += usize::from(current_section.is_some());
        }
        if index == selected {
            return row;
        }
        row += 1;
    }
    0
}

/// Item index shown at `row_index` in row space, or `None` for section
/// headers and out-of-range rows.
pub(super) fn item_index_at_row(
    items: &[PickerItem],
    matching: &[usize],
    row_index: usize,
) -> Option<usize> {
    let mut row = 0;
    let mut current_section: Option<&str> = None;
    for index in matching.iter().copied() {
        let item = items.get(index)?;
        if item.section.as_deref() != current_section {
            current_section = item.section.as_deref();
            if current_section.is_some() {
                if row == row_index {
                    return None;
                }
                row += 1;
            }
        }
        if row == row_index {
            return Some(index);
        }
        row += 1;
    }
    None
}

/// Build the rows for the matching items, inserting a header row whenever the
/// section changes. `hovered_row` highlights that row-space row under the
/// pointer.
pub(super) fn picker_item_rows(
    items: &[PickerItem],
    matching: &[usize],
    selected: usize,
    layout: RowLayout,
    hovered_row: Option<usize>,
) -> PickerRows {
    let mut rows = Vec::with_capacity(matching.len());
    let mut current_section = None;
    let mut selected_row = 0;
    for index in matching.iter().copied() {
        let Some(item) = items.get(index) else {
            continue;
        };
        if item.section.as_deref() != current_section {
            current_section = item.section.as_deref();
            if let Some(section) = current_section {
                rows.push(section_header_line(section, layout));
            }
        }
        if index == selected {
            selected_row = rows.len();
        }
        let hovered = hovered_row == Some(rows.len());
        rows.push(item_line(item, index == selected, hovered, layout));
    }
    PickerRows { rows, selected_row }
}

/// First visible row when `viewport_rows` rows show and the selected row must
/// stay on screen.
pub(super) fn scroll_window_start(selected_row: usize, viewport_rows: usize) -> usize {
    selected_row
        .saturating_add(1)
        .saturating_sub(viewport_rows.max(1))
}

fn section_header_line(section: &str, layout: RowLayout) -> Line<'static> {
    let label = truncate_one_line(section, layout.width.saturating_sub(2));
    styled_line(
        format!("  {label}"),
        layout.width,
        Theme::dim(),
        layout.fill,
    )
}

fn item_line(item: &PickerItem, selected: bool, hovered: bool, layout: RowLayout) -> Line<'static> {
    let width = layout.width;
    if width == 0 {
        return Line::raw("");
    }
    let marker = if selected {
        super::composer_chrome::SELECTION_MARKER_ACTIVE
    } else {
        super::composer_chrome::SELECTION_MARKER_INACTIVE
    };
    let style = if selected {
        Theme::accent()
    } else if hovered {
        Theme::text_strong()
    } else {
        Theme::text()
    };
    if width == 1 {
        return Line::from(Span::styled(marker.to_string(), style));
    }

    let available = width.saturating_sub(2);
    match layout.width_mode {
        RowWidthMode::FillPane => fill_pane_line(item, marker, style, available, layout),
        RowWidthMode::AlignedColumn(column) => {
            aligned_column_line(item, marker, style, column, layout)
        }
    }
}

/// Overlay nav row: badge keeps its width, label pads through the remainder.
fn fill_pane_line(
    item: &PickerItem,
    marker: &str,
    style: Style,
    available: usize,
    layout: RowLayout,
) -> Line<'static> {
    let badge = layout
        .show_badges
        .then_some(item.badge.as_ref())
        .flatten()
        .and_then(|badge| {
            let budget = display_width(&badge.text)
                .min(NAV_BADGE_CAP)
                .min(available.saturating_sub(2));
            (budget > 0).then(|| (truncate_one_line(&badge.text, budget), badge.tone))
        });
    let badge_width = badge
        .as_ref()
        .map_or(0, |(text, _)| display_width(text).saturating_add(1));
    let label_budget = available.saturating_sub(badge_width);
    let label = truncate_one_line(&item.label, label_budget);
    let mut spans = vec![Span::styled(
        format!(
            "{marker} {label}{}",
            " ".repeat(label_budget.saturating_sub(display_width(&label)))
        ),
        style,
    )];
    if let Some((text, tone)) = badge {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(text, picker_badge_style(tone)));
    }
    Line::from(spans)
}

/// Inline list row: label column, then badge, then preview in the free width.
fn aligned_column_line(
    item: &PickerItem,
    marker: &str,
    style: Style,
    column: usize,
    layout: RowLayout,
) -> Line<'static> {
    let width = layout.width;
    let label_width = column.min(width.saturating_sub(2));
    let label = truncate_one_line(&item.label, label_width);
    let mut used_width = 2 + label_width;
    let mut spans = vec![Span::styled(
        format!(
            "{marker} {label}{}",
            " ".repeat(label_width.saturating_sub(display_width(&label)))
        ),
        style,
    )];
    if layout.show_badges {
        if let Some(badge) = &item.badge {
            let remaining = width.saturating_sub(used_width.saturating_add(2));
            if remaining > 1 {
                // Badges use free width instead of a magic cap; preview text,
                // when present, takes whatever remains after the badge.
                let badge_text = truncate_one_line(&badge.text, remaining);
                used_width += 2 + display_width(&badge_text);
                spans.push(Span::raw("  "));
                spans.push(Span::styled(badge_text, picker_badge_style(badge.tone)));
            }
        }
    }
    if layout.show_preview {
        if let Some(preview) = &item.preview {
            let remaining = width.saturating_sub(used_width.saturating_add(2));
            if remaining > 1 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    truncate_one_line(preview, remaining),
                    Theme::dim(),
                ));
            }
        }
    }
    if layout.fill.pads_to_width() {
        let content_width = spans
            .iter()
            .map(|span| display_width(span.content.as_ref()))
            .sum::<usize>();
        if content_width < width {
            spans.push(Span::raw(" ".repeat(width - content_width)));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "picker_rows_tests.rs"]
mod tests;
