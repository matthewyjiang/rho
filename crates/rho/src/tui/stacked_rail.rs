//! Shared overflow and pointer machine for stacked activity rails.
//!
//! The rail has no clock. A finished row stays exactly while its host keeps
//! serving it, and hosts stop serving it when its result is delivered, so the
//! row leaves in the same repaint that lands the result in the transcript.

use ratatui::layout::{Position, Rect};

use super::activity::{self, RailRowState};

pub(super) trait RailItem {
    fn id(&self) -> &str;
    fn is_live(&self) -> bool;
    fn is_failure(&self) -> bool;
}

/// What a pointer hit on a capped rail means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RailHit {
    Item(String),
    Overflow,
}

/// Which rows accept hover, press, and activate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RailPointerPolicy {
    /// Finished, undelivered rows stay clickable. Overflow is display-only.
    LiveOrFinished,
    /// Only live rows activate. Overflow uses `overflow_id`.
    LiveAndOverflow { overflow_id: &'static str },
}

#[derive(Clone, Debug)]
pub(super) struct StackedRail<T> {
    items: Vec<T>,
    hovered_id: Option<String>,
    pressed_id: Option<String>,
    pointer: RailPointerPolicy,
}

impl<T> StackedRail<T> {
    pub(super) fn new(pointer: RailPointerPolicy) -> Self {
        Self {
            items: Vec::new(),
            hovered_id: None,
            pressed_id: None,
            pointer,
        }
    }

    pub(super) fn items(&self) -> &[T] {
        &self.items
    }

    pub(super) fn is_active(&self) -> bool {
        !self.items.is_empty()
    }

    pub(super) fn desired_height(&self) -> usize {
        self.items.len().min(activity::MAX_VISIBLE_RAIL_ROWS)
    }

    pub(super) fn clear_pointer_state(&mut self) {
        self.hovered_id = None;
        self.pressed_id = None;
    }

    pub(super) fn clear_pressed(&mut self) {
        self.pressed_id = None;
    }

    pub(super) fn pressed_id(&self) -> Option<&str> {
        self.pressed_id.as_deref()
    }

    /// Returns whether the hovered id changed.
    pub(super) fn set_hovered(&mut self, id: Option<&str>) -> bool {
        if self.hovered_id.as_deref() == id {
            return false;
        }
        self.hovered_id = id.map(str::to_owned);
        true
    }

    /// Returns whether the pressed id changed.
    pub(super) fn set_pressed(&mut self, id: Option<&str>) -> bool {
        if self.pressed_id.as_deref() == id {
            return false;
        }
        self.pressed_id = id.map(str::to_owned);
        true
    }
}

impl<T: RailItem + PartialEq> StackedRail<T> {
    /// Replace the rows with the host's current snapshot. Returns whether
    /// anything changed.
    pub(super) fn ingest(&mut self, items: Vec<T>) -> bool {
        if self.items == items {
            return false;
        }
        self.items = items;
        if !self
            .hovered_id
            .as_deref()
            .is_some_and(|id| self.pointer_active(id))
        {
            self.hovered_id = None;
        }
        if !self
            .pressed_id
            .as_deref()
            .is_some_and(|id| self.pointer_active(id))
        {
            self.pressed_id = None;
        }
        true
    }

    fn pointer_active(&self, id: &str) -> bool {
        match self.pointer {
            RailPointerPolicy::LiveOrFinished => self.items.iter().any(|item| item.id() == id),
            RailPointerPolicy::LiveAndOverflow { overflow_id } => {
                (id == overflow_id && self.overflow_active())
                    || self
                        .items
                        .iter()
                        .any(|item| item.id() == id && item.is_live())
            }
        }
    }

    fn overflow_active(&self) -> bool {
        self.items.len() > activity::MAX_VISIBLE_RAIL_ROWS
    }
}

impl<T: RailItem> StackedRail<T> {
    pub(super) fn live_count(&self) -> usize {
        self.items.iter().filter(|item| item.is_live()).count()
    }

    pub(super) fn live_items(&self) -> impl Iterator<Item = &T> {
        self.items.iter().filter(|item| item.is_live())
    }

    pub(super) fn highlighted_row(&self, height: usize) -> Option<(usize, RailRowState)> {
        let (rows, hidden) = self.visible(height);
        let row_for = |id: &str| {
            rows.iter()
                .position(|item| item.id() == id)
                .or_else(|| match (hidden, self.pointer) {
                    (Some(_), RailPointerPolicy::LiveAndOverflow { overflow_id })
                        if id == overflow_id =>
                    {
                        Some(rows.len())
                    }
                    _ => None,
                })
        };
        if let Some(row) = self.pressed_id.as_deref().and_then(row_for) {
            return Some((row, RailRowState::Pressed));
        }
        self.hovered_id
            .as_deref()
            .and_then(row_for)
            .map(|row| (row, RailRowState::Hovered))
    }

    pub(super) fn hit_at(&self, area: Rect, column: u16, row: u16) -> Option<RailHit> {
        if !area.contains(Position { x: column, y: row }) || area.height == 0 {
            return None;
        }
        let index = row.saturating_sub(area.y) as usize;
        let (rows, hidden) = self.visible(area.height as usize);
        if index < rows.len() {
            let item = rows[index];
            return match self.pointer {
                RailPointerPolicy::LiveOrFinished => Some(RailHit::Item(item.id().to_owned())),
                RailPointerPolicy::LiveAndOverflow { .. } if item.is_live() => {
                    Some(RailHit::Item(item.id().to_owned()))
                }
                RailPointerPolicy::LiveAndOverflow { .. } => None,
            };
        }
        if hidden.is_some() && index == rows.len() {
            return match self.pointer {
                RailPointerPolicy::LiveAndOverflow { .. } => Some(RailHit::Overflow),
                RailPointerPolicy::LiveOrFinished => None,
            };
        }
        None
    }

    pub(super) fn visible(&self, height: usize) -> (Vec<&T>, Option<usize>) {
        let (indices, hidden) = activity::select_capped_rail_rows(
            &self.items,
            height,
            |item| item.is_live(),
            |item| item.is_failure(),
        );
        (
            indices
                .into_iter()
                .map(|index| &self.items[index])
                .collect(),
            hidden,
        )
    }

    pub(super) fn row_state(&self, id: &str, live: bool) -> RailRowState {
        if matches!(self.pointer, RailPointerPolicy::LiveAndOverflow { .. }) && !live {
            return RailRowState::Idle;
        }
        if self.pressed_id.as_deref() == Some(id) {
            RailRowState::Pressed
        } else if self.hovered_id.as_deref() == Some(id) {
            RailRowState::Hovered
        } else {
            RailRowState::Idle
        }
    }

    pub(super) fn overflow_row_state(&self) -> RailRowState {
        match self.pointer {
            RailPointerPolicy::LiveAndOverflow { overflow_id } => {
                self.row_state(overflow_id, /*live*/ true)
            }
            RailPointerPolicy::LiveOrFinished => RailRowState::Idle,
        }
    }
}
