//! Prompt-history stepping shared by the main composer and the side chat.
//! Owners keep their own entries and drafts; this decides where Up/Down go.

use super::HistoryDirection;

/// Where a history key moves from the current position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HistoryStep {
    /// Show entry `index`. `save_draft` is set when leaving the live draft.
    Recall { index: usize, save_draft: bool },
    /// Step past the newest entry back to the saved live draft.
    RestoreDraft,
}

/// Step `direction` through `len` entries from `cursor` (`None` is the live
/// draft). `None` means the key does not touch history.
pub(super) fn history_step(
    direction: HistoryDirection,
    len: usize,
    cursor: Option<usize>,
) -> Option<HistoryStep> {
    if len == 0 {
        return None;
    }
    let step = match (direction, cursor) {
        (HistoryDirection::Previous, None) => HistoryStep::Recall {
            index: len - 1,
            save_draft: true,
        },
        (HistoryDirection::Previous, Some(cursor)) => HistoryStep::Recall {
            index: cursor.saturating_sub(1),
            save_draft: false,
        },
        (HistoryDirection::Next, None) => return None,
        (HistoryDirection::Next, Some(cursor)) if cursor + 1 < len => HistoryStep::Recall {
            index: cursor + 1,
            save_draft: false,
        },
        (HistoryDirection::Next, Some(_)) => HistoryStep::RestoreDraft,
    };
    Some(step)
}
