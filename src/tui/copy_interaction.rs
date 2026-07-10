use std::{ops::Range, sync::Arc};

use ratatui::layout::{Position, Rect};

use super::text_selection::SelectionPosition;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodeBlockCopyTarget {
    pub(super) line: usize,
    pub(super) columns: Range<usize>,
    pub(super) text: Arc<str>,
}

pub(super) fn code_block_copy_target_at(
    targets: &[CodeBlockCopyTarget],
    history: Rect,
    history_start: usize,
    column: u16,
    row: u16,
) -> Option<CodeBlockCopyTarget> {
    if !history.contains(Position { x: column, y: row }) {
        return None;
    }
    let line = history_start.saturating_add(row.saturating_sub(history.y) as usize);
    let relative_column = column.saturating_sub(history.x) as usize;
    targets
        .iter()
        .find(|target| target.line == line && target.columns.contains(&relative_column))
        .cloned()
}

pub(super) fn selection_position(
    history: Rect,
    history_start: usize,
    column: u16,
    row: u16,
) -> Option<SelectionPosition> {
    history
        .contains(Position { x: column, y: row })
        .then(|| SelectionPosition {
            line: history_start.saturating_add(row.saturating_sub(history.y) as usize),
            column: column.saturating_sub(history.x) as usize,
        })
}

pub(super) fn selection_position_clamped(
    history: Rect,
    history_start: usize,
    column: u16,
    row: u16,
) -> Option<SelectionPosition> {
    if history.width == 0 || history.height == 0 {
        return None;
    }
    let column = column.clamp(
        history.x,
        history.x.saturating_add(history.width.saturating_sub(1)),
    );
    let row = row.clamp(
        history.y,
        history.y.saturating_add(history.height.saturating_sub(1)),
    );
    Some(SelectionPosition {
        line: history_start.saturating_add(row.saturating_sub(history.y) as usize),
        column: column.saturating_sub(history.x) as usize,
    })
}
