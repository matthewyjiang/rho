//! Shared linger, overflow, and pointer machine for stacked activity rails.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use ratatui::layout::{Position, Rect};

use super::activity::{self, RailRowState};

pub(super) trait RailItem {
    fn id(&self) -> &str;
    fn is_live(&self) -> bool;
    fn is_failure(&self) -> bool;
    fn linger(&self) -> Duration;
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
    /// Lingering rows stay clickable. Overflow is display-only.
    LiveOrLinger,
    /// Only live rows activate. Overflow uses `overflow_id`.
    LiveAndOverflow { overflow_id: &'static str },
}

#[derive(Clone, Debug)]
pub(super) struct LingerRail<T> {
    items: Vec<T>,
    terminal_seen: HashMap<String, Instant>,
    hovered_id: Option<String>,
    pressed_id: Option<String>,
    pointer: RailPointerPolicy,
}

impl<T> LingerRail<T> {
    pub(super) fn new(pointer: RailPointerPolicy) -> Self {
        Self {
            items: Vec::new(),
            terminal_seen: HashMap::new(),
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

impl<T: RailItem + PartialEq> LingerRail<T> {
    pub(super) fn ingest(&mut self, items: Vec<T>, now: Instant) -> bool {
        let incoming: HashSet<String> = items.iter().map(|item| item.id().to_owned()).collect();
        self.terminal_seen.retain(|id, _| incoming.contains(id));
        let items = items
            .into_iter()
            .filter(|item| self.keep(item, now))
            .collect();
        self.replace(items)
    }

    fn keep(&mut self, item: &T, now: Instant) -> bool {
        if item.is_live() {
            self.terminal_seen.remove(item.id());
            return true;
        }
        let first_seen = *self
            .terminal_seen
            .entry(item.id().to_owned())
            .or_insert(now);
        activity::linger_active(first_seen, now, item.linger())
    }

    fn replace(&mut self, items: Vec<T>) -> bool {
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
            RailPointerPolicy::LiveOrLinger => self.items.iter().any(|item| item.id() == id),
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

impl<T: RailItem> LingerRail<T> {
    pub(super) fn live_count(&self) -> usize {
        self.items.iter().filter(|item| item.is_live()).count()
    }

    pub(super) fn live_items(&self) -> impl Iterator<Item = &T> {
        self.items.iter().filter(|item| item.is_live())
    }

    pub(super) fn highlighted_row(
        &self,
        height: usize,
        now: Instant,
    ) -> Option<(usize, RailRowState)> {
        let (rows, hidden) = self.visible(height, now);
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

    pub(super) fn hit_at(
        &self,
        area: Rect,
        column: u16,
        row: u16,
        now: Instant,
    ) -> Option<RailHit> {
        if !area.contains(Position { x: column, y: row }) || area.height == 0 {
            return None;
        }
        let index = row.saturating_sub(area.y) as usize;
        let (rows, hidden) = self.visible(area.height as usize, now);
        if index < rows.len() {
            let item = rows[index];
            return match self.pointer {
                RailPointerPolicy::LiveOrLinger => Some(RailHit::Item(item.id().to_owned())),
                RailPointerPolicy::LiveAndOverflow { .. } if item.is_live() => {
                    Some(RailHit::Item(item.id().to_owned()))
                }
                RailPointerPolicy::LiveAndOverflow { .. } => None,
            };
        }
        if hidden.is_some() && index == rows.len() {
            return match self.pointer {
                RailPointerPolicy::LiveAndOverflow { .. } => Some(RailHit::Overflow),
                RailPointerPolicy::LiveOrLinger => None,
            };
        }
        None
    }

    pub(super) fn visible(&self, height: usize, now: Instant) -> (Vec<&T>, Option<usize>) {
        let items: Vec<&T> = self
            .items
            .iter()
            .filter(|item| self.row_visible(item, now))
            .collect();
        let (indices, hidden) = activity::select_capped_rail_rows(
            &items,
            height,
            |item| item.is_live(),
            |item| item.is_failure(),
        );
        (
            indices.into_iter().map(|index| items[index]).collect(),
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
            RailPointerPolicy::LiveOrLinger => RailRowState::Idle,
        }
    }

    fn row_visible(&self, item: &T, now: Instant) -> bool {
        if item.is_live() {
            return true;
        }
        self.terminal_seen
            .get(item.id())
            .is_some_and(|first| activity::linger_active(*first, now, item.linger()))
    }
}
