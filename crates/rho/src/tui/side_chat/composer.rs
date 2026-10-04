//! Side chat composer: the main composer's buffer and key table, with its own
//! in-memory prompt history. Asides never write to the parent's history.

use std::time::Instant;

use super::super::{
    click_sequence::ClickSequence,
    composer_buffer::{ComposerBuffer, ComposerEditKey, EditOutcome},
    composer_history::{history_step, HistoryStep},
    paste_burst::{collapsed_paste_for, CollapsedPaste},
    HistoryDirection, PasteSegment,
};

/// What the caller still owes a side edit key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SideEditFollowUp {
    None,
    /// Up/Down hit an edge of an empty composer with no history to recall,
    /// so the key scrolls the transcript instead.
    ScrollTranscript(HistoryDirection),
}

#[derive(Debug, Default)]
pub(super) struct SideComposer {
    pub(super) buffer: ComposerBuffer,
    history: Vec<String>,
    /// Recalled entry; `None` while editing the live draft.
    history_cursor: Option<usize>,
    history_draft: Option<(String, Vec<PasteSegment>)>,
    clicks: ClickSequence,
    /// Wrapped text width at the last paint; vertical moves follow it.
    painted_width: usize,
}

impl SideComposer {
    pub(super) fn set_painted_width(&mut self, width: usize) {
        self.painted_width = width;
    }

    /// Apply a shared edit key, recalling side history at the top/bottom row.
    pub(super) fn apply_edit(&mut self, edit: ComposerEditKey) -> SideEditFollowUp {
        if matches!(edit, ComposerEditKey::Home | ComposerEditKey::End) {
            self.reset_history_navigation();
        }
        let width = self.painted_width.max(1);
        match self.buffer.apply_edit(edit, width) {
            EditOutcome::Edited => self.reset_history_navigation(),
            EditOutcome::VerticalEdge(direction) => {
                if self.recall_history(direction) {
                    return SideEditFollowUp::None;
                }
                if self.buffer.is_empty() {
                    return SideEditFollowUp::ScrollTranscript(direction);
                }
                self.buffer.move_vertically(direction, width);
            }
            EditOutcome::Moved | EditOutcome::Unchanged => {}
        }
        SideEditFollowUp::None
    }

    /// Insert pasted text, collapsing a large paste into a marker. Returns
    /// the collapsed paste so the caller can confirm it.
    pub(super) fn insert_paste(&mut self, text: &str) -> Option<CollapsedPaste> {
        self.clicks.cancel();
        self.reset_history_navigation();
        let Some(paste) = collapsed_paste_for(text) else {
            self.buffer.insert_text(text);
            return None;
        };
        self.buffer.insert_collapsed_paste(&paste, text);
        Some(paste)
    }

    /// Replace the whole draft, for example with external editor output.
    pub(super) fn replace_text(&mut self, text: String) {
        self.reset_history_navigation();
        self.buffer.replace_all(text, Vec::new());
    }

    pub(super) fn clear(&mut self) {
        self.reset_history_navigation();
        self.buffer.clear();
    }

    /// Take the expanded draft for submission and remember it for recall.
    pub(super) fn take_submission(&mut self) -> String {
        let text = self.buffer.take_expanded();
        self.reset_history_navigation();
        let prompt = text.trim();
        if !prompt.is_empty() && self.history.last().map(String::as_str) != Some(prompt) {
            self.history.push(prompt.to_owned());
        }
        text
    }

    /// Primary press on raw char index `index` at screen cell `column`/`row`.
    pub(super) fn pointer_press(&mut self, index: usize, now: Instant, column: u16, row: u16) {
        self.reset_history_navigation();
        let index = self.buffer.caret_index(index);
        let double_click = self.clicks.register(now, column, row, index);
        self.buffer.pointer_press(index, double_click);
    }

    /// Forget a pending double click; any key breaks the sequence.
    pub(super) fn cancel_click_sequence(&mut self) {
        self.clicks.cancel();
    }

    /// Extend a drag selection; a drag never pairs into a double click.
    pub(super) fn pointer_drag(&mut self, index: usize) {
        self.clicks.cancel();
        self.buffer.pointer_drag(index);
    }

    pub(super) fn pointer_release(&mut self, index: Option<usize>) {
        self.buffer.pointer_release(index);
    }

    /// Drop the selection and any half-finished double click, for presses
    /// that land outside the composer.
    pub(super) fn cancel_pointer(&mut self) {
        self.buffer.clear_selection();
        self.clicks.cancel();
    }

    /// End pointer interaction without a release (focus change, overlay
    /// close): keep a dragged selection, forget a pending double click.
    pub(super) fn settle_pointer(&mut self) {
        self.clicks.cancel();
        self.buffer.finalize_selection();
    }

    fn reset_history_navigation(&mut self) {
        self.history_cursor = None;
        self.history_draft = None;
    }

    fn recall_history(&mut self, direction: HistoryDirection) -> bool {
        let Some(step) = history_step(direction, self.history.len(), self.history_cursor) else {
            return false;
        };
        match step {
            HistoryStep::Recall { index, save_draft } => {
                if save_draft {
                    self.history_draft = Some((
                        self.buffer.text().to_owned(),
                        self.buffer.paste_segments().to_vec(),
                    ));
                }
                self.buffer
                    .replace_all(self.history[index].clone(), Vec::new());
                self.history_cursor = Some(index);
            }
            HistoryStep::RestoreDraft => {
                let (text, segments) = self.history_draft.take().unwrap_or_default();
                self.buffer.replace_all(text, segments);
                self.history_cursor = None;
            }
        }
        true
    }
}
