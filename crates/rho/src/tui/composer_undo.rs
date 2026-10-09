//! Undo history for [`ComposerBuffer`](super::composer_buffer::ComposerBuffer).
//!
//! Every text edit is an invertible [`Splice`], so history memory follows
//! what the user typed or deleted instead of snapshotting the whole draft,
//! and any large collapsed paste, on every step. Typing coalesces into one
//! step per word and a run of Backspace or Delete into one step; an explicit
//! group (a vim insert session or normal-mode command) folds every splice
//! into a single step.

use super::PasteSegment;

/// One text replacement at char `start`: `removed`, which held
/// `removed_segments`, became `inserted`, which holds `inserted_segments`.
/// Segment positions are absolute in the text they belong to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Splice {
    pub(super) start: usize,
    pub(super) removed: String,
    pub(super) removed_segments: Vec<PasteSegment>,
    pub(super) inserted: String,
    pub(super) inserted_segments: Vec<PasteSegment>,
}

impl Splice {
    /// The splice that turns the edited text back into the original.
    pub(super) fn inverse(&self) -> Self {
        Self {
            start: self.start,
            removed: self.inserted.clone(),
            removed_segments: self.inserted_segments.clone(),
            inserted: self.removed.clone(),
            inserted_segments: self.removed_segments.clone(),
        }
    }

    pub(super) fn is_noop(&self) -> bool {
        self.removed.is_empty() && self.inserted.is_empty()
    }

    fn is_plain(&self) -> bool {
        self.removed_segments.is_empty() && self.inserted_segments.is_empty()
    }

    fn single_inserted_char(&self) -> Option<char> {
        let mut chars = self.inserted.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) if self.removed.is_empty() && self.is_plain() => Some(ch),
            _ => None,
        }
    }

    fn is_single_char_deletion(&self) -> bool {
        self.inserted.is_empty() && self.is_plain() && self.removed.chars().count() == 1
    }

    /// Fold `next`, which happened right after `self`, into `self` when both
    /// belong to one typing or deletion run. Returns false when they do not.
    fn absorb(&mut self, next: &Splice) -> bool {
        if !self.is_plain() {
            return false;
        }
        if let Some(ch) = next.single_inserted_char() {
            let run_end = self.start + self.inserted.chars().count();
            // A word starts a new step: typing "foo bar" undoes "bar" first.
            let starts_word = !ch.is_whitespace()
                && self
                    .inserted
                    .chars()
                    .last()
                    .is_some_and(char::is_whitespace);
            if self.removed.is_empty() && next.start == run_end && !starts_word {
                self.inserted.push(ch);
                return true;
            }
            return false;
        }
        if next.is_single_char_deletion() && self.inserted.is_empty() {
            if next.start + 1 == self.start {
                // Backspace: the deleted char precedes the run.
                self.start = next.start;
                self.removed.insert_str(0, &next.removed);
                return true;
            }
            if next.start == self.start {
                // Delete: the deleted char follows the run.
                self.removed.push_str(&next.removed);
                return true;
            }
        }
        false
    }
}

/// Splices that undo or redo together, with the caret on each side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UndoStep {
    pub(super) splices: Vec<Splice>,
    pub(super) cursor_before: usize,
    pub(super) cursor_after: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Grouping {
    /// Each splice starts a step unless it extends a typing run.
    #[default]
    Ungrouped,
    /// An explicit group is open; its first splice has not landed yet.
    Pending,
    /// An explicit group is open and owns the top undo step.
    Open,
}

/// Undo and redo stacks. Unbounded: each step holds only the text its edits
/// touched, which the user had to type, paste, or delete.
#[derive(Clone, Debug, Default)]
pub(super) struct UndoHistory {
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    grouping: Grouping,
    /// Whether the top undo step may absorb the next typing splice.
    run_open: bool,
}

impl UndoHistory {
    /// Record an applied splice. Any new edit clears the redo stack.
    pub(super) fn record(&mut self, splice: Splice, cursor_before: usize, cursor_after: usize) {
        if splice.is_noop() {
            return;
        }
        self.redo.clear();
        match self.grouping {
            Grouping::Open => {
                if let Some(step) = self.undo.last_mut() {
                    step.splices.push(splice);
                    step.cursor_after = cursor_after;
                    return;
                }
            }
            Grouping::Pending => self.grouping = Grouping::Open,
            Grouping::Ungrouped => {
                if self.run_open {
                    if let Some(step) = self.undo.last_mut() {
                        if let [last] = step.splices.as_mut_slice() {
                            if last.absorb(&splice) {
                                step.cursor_after = cursor_after;
                                return;
                            }
                        }
                    }
                }
                self.run_open = true;
            }
        }
        self.undo.push(UndoStep {
            splices: vec![splice],
            cursor_before,
            cursor_after,
        });
    }

    /// Fold every splice until [`Self::end_group`] into one undo step.
    pub(super) fn begin_group(&mut self) {
        self.grouping = Grouping::Pending;
        self.run_open = false;
    }

    pub(super) fn end_group(&mut self) {
        self.grouping = Grouping::Ungrouped;
        self.run_open = false;
    }

    /// Pop the newest step for the caller to revert, then hand it to
    /// [`Self::push_redo`]. Closes any open group first.
    pub(super) fn pop_undo(&mut self) -> Option<UndoStep> {
        self.end_group();
        self.undo.pop()
    }

    pub(super) fn push_redo(&mut self, step: UndoStep) {
        self.redo.push(step);
    }

    /// Pop the newest undone step for the caller to reapply, then hand it to
    /// [`Self::push_undo`].
    pub(super) fn pop_redo(&mut self) -> Option<UndoStep> {
        self.end_group();
        self.redo.pop()
    }

    pub(super) fn push_undo(&mut self, step: UndoStep) {
        self.undo.push(step);
    }
}
