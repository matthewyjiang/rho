//! Double-click detection shared by every pointer surface.

use std::time::{Duration, Instant};

/// Max gap between presses that still counts as a double click.
pub(super) const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(500);

/// The last unpaired press. A second press pairs with it only on the same
/// cell and the same logical target (`index`) within [`DOUBLE_CLICK_GAP`], so
/// a list that shifts between presses never pairs two different rows.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ClickSequence {
    last: Option<Press>,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    at: Instant,
    column: u16,
    row: u16,
    index: usize,
}

impl ClickSequence {
    /// Records a press and returns whether it completes a double click. A
    /// completed double click starts a fresh sequence, so a third press is a
    /// single click again.
    pub(super) fn register(&mut self, now: Instant, column: u16, row: u16, index: usize) -> bool {
        let double = self.last.is_some_and(|press| {
            now.saturating_duration_since(press.at) <= DOUBLE_CLICK_GAP
                && press.column == column
                && press.row == row
                && press.index == index
        });
        self.last = (!double).then_some(Press {
            at: now,
            column,
            row,
            index,
        });
        double
    }

    pub(super) fn cancel(&mut self) {
        self.last = None;
    }
}
