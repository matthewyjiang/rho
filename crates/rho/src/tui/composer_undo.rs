//! Undo history for [`ComposerBuffer`](super::composer_buffer::ComposerBuffer).
//!
//! Every text edit is an invertible [`Splice`], so history memory follows
//! what the user typed or deleted instead of snapshotting the whole draft,
//! and any large collapsed paste, on every step. Typing coalesces into one
//! step per word and a run of Backspace or Delete into one step; an explicit
//! group (a vim insert session or normal-mode command) folds every splice
//! into a single step.

use super::composer_buffer::Fragment;

/// One text replacement at char `start`; markers are relative to each fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Splice {
    pub(super) start: usize,
    pub(super) removed: Fragment,
    pub(super) inserted: Fragment,
}

impl Splice {
    /// The splice that turns the edited text back into the original.
    pub(super) fn inverse(&self) -> Self {
        Self {
            start: self.start,
            removed: self.inserted.clone(),
            inserted: self.removed.clone(),
        }
    }

    pub(super) fn is_noop(&self) -> bool {
        self.removed.text.is_empty() && self.inserted.text.is_empty()
    }

    fn is_plain(&self) -> bool {
        self.removed.segments.is_empty() && self.inserted.segments.is_empty()
    }

    fn single_inserted_char(&self) -> Option<char> {
        let mut chars = self.inserted.text.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) if self.removed.text.is_empty() && self.is_plain() => Some(ch),
            _ => None,
        }
    }

    fn is_single_char_deletion(&self) -> bool {
        self.inserted.text.is_empty() && self.is_plain() && self.removed.text.chars().count() == 1
    }

    /// Fold `next`, which happened right after `self`, into `self` when both
    /// belong to one typing or deletion run. Returns false when they do not.
    fn absorb(&mut self, next: &Splice) -> bool {
        if !self.is_plain() {
            return false;
        }
        if let Some(ch) = next.single_inserted_char() {
            let run_end = self.start + self.inserted.text.chars().count();
            // A word starts a new step: typing "foo bar" undoes "bar" first.
            let starts_word = !ch.is_whitespace()
                && self
                    .inserted
                    .text
                    .chars()
                    .last()
                    .is_some_and(char::is_whitespace);
            if self.removed.text.is_empty() && next.start == run_end && !starts_word {
                self.inserted.text.push(ch);
                return true;
            }
            return false;
        }
        if next.is_single_char_deletion() && self.inserted.text.is_empty() {
            if next.start + 1 == self.start {
                // Backspace: the deleted char precedes the run.
                self.start = next.start;
                self.removed.text.insert_str(0, &next.removed.text);
                return true;
            }
            if next.start == self.start {
                // Delete: the deleted char follows the run.
                self.removed.text.push_str(&next.removed.text);
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
    coalesce: Coalesce,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Coalesce {
    #[default]
    Never,
    Typing,
    Deletion,
}

/// Undo and redo stacks. Unbounded: each step holds only the text its edits
/// touched, which the user had to type, paste, or delete.
#[derive(Clone, Debug, Default)]
pub(super) struct UndoHistory {
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    group_depth: usize,
    /// The next edit starts a fresh step, even inside a group.
    sealed: bool,
}

impl UndoHistory {
    /// Record an applied splice. Any new edit clears the redo stack.
    pub(super) fn record(
        &mut self,
        splice: Splice,
        cursor_before: usize,
        cursor_after: usize,
        coalesce: Coalesce,
    ) {
        if splice.is_noop() {
            return;
        }
        self.redo.clear();
        let open_step = if self.sealed {
            None
        } else {
            self.undo.last_mut()
        };
        if let Some(step) = open_step {
            if self.group_depth > 0 {
                step.splices.push(splice);
                step.cursor_after = cursor_after;
                return;
            }
            if coalesce != Coalesce::Never
                && step.coalesce == coalesce
                && match step.splices.as_mut_slice() {
                    [last] => last.absorb(&splice),
                    _ => false,
                }
            {
                step.cursor_after = cursor_after;
                return;
            }
        }
        self.undo.push(UndoStep {
            splices: vec![splice],
            cursor_before,
            cursor_after,
            coalesce,
        });
        self.sealed = false;
    }

    /// Fold every splice until [`Self::end_group`] into one undo step.
    pub(super) fn begin_group(&mut self) {
        if self.group_depth == 0 {
            self.sealed = true;
        }
        self.group_depth += 1;
    }

    pub(super) fn end_group(&mut self) {
        self.group_depth = self.group_depth.saturating_sub(1);
        if self.group_depth == 0 {
            self.sealed = true;
        }
    }

    /// Pop the newest step for the caller to revert, then hand it to
    /// [`Self::push_redo`]. Seals the step without ending its owner's group.
    pub(super) fn pop_undo(&mut self) -> Option<UndoStep> {
        self.sealed = true;
        self.undo.pop()
    }

    pub(super) fn push_redo(&mut self, step: UndoStep) {
        self.redo.push(step);
    }

    /// Pop the newest undone step for the caller to reapply, then hand it to
    /// [`Self::push_undo`].
    pub(super) fn pop_redo(&mut self) -> Option<UndoStep> {
        self.sealed = true;
        self.redo.pop()
    }

    pub(super) fn push_undo(&mut self, step: UndoStep) {
        self.undo.push(step);
    }
}
