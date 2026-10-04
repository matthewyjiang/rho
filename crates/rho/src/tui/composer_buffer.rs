//! Editable composer text shared by the main composer and the side chat.
//!
//! [`ComposerBuffer`] owns text, caret, mouse selection, collapsed paste
//! markers, and the painted row window. It knows nothing about palettes,
//! attachments, shell mode, or history; owners apply those around its edits.
//! [`ComposerEditKey`] is the one key table both composers dispatch through.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    paste_burst::{next_word_boundary, previous_word_boundary, word_range_at, CollapsedPaste},
    render::{
        editable_input_visual_lines, input_char_index_at_position,
        input_cursor_index_on_visual_line, input_frame, visual_caret_position, InputFrame,
    },
    HistoryDirection, PasteSegment,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerSelection {
    Characters {
        anchor: usize,
        focus: usize,
    },
    Range {
        start: usize,
        end: usize,
        focus: usize,
    },
}

impl ComposerSelection {
    fn characters(position: usize) -> Self {
        Self::Characters {
            anchor: position,
            focus: position,
        }
    }

    fn range(start: usize, end: usize) -> Self {
        Self::Range {
            start,
            end,
            focus: end,
        }
    }

    fn update(&mut self, position: usize) {
        match self {
            Self::Characters { focus, .. } | Self::Range { focus, .. } => *focus = position,
        }
    }

    fn pointer_origin(self) -> usize {
        match self {
            Self::Characters { anchor, .. } => anchor,
            Self::Range { start, .. } => start,
        }
    }

    fn focus(self) -> usize {
        match self {
            Self::Characters { focus, .. } => focus,
            Self::Range {
                start, end, focus, ..
            } => {
                if focus < start || focus > end {
                    focus
                } else {
                    end
                }
            }
        }
    }

    /// Ordered half-open char range when the selection spans text.
    fn edit_range(self) -> Option<std::ops::Range<usize>> {
        match self {
            Self::Characters { anchor, focus } if anchor < focus => Some(anchor..focus),
            Self::Characters { anchor, focus } if focus < anchor => Some(focus..anchor),
            Self::Characters { .. } => None,
            Self::Range {
                start, end, focus, ..
            } if focus < start => Some(focus..end),
            Self::Range {
                start, end, focus, ..
            } if focus > end => Some(start..focus),
            Self::Range { start, end, .. } if start < end => Some(start..end),
            Self::Range { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ComposerSelectionState {
    #[default]
    None,
    Dragging(ComposerSelection),
    Selected(ComposerSelection),
}

impl ComposerSelectionState {
    fn value(self) -> Option<ComposerSelection> {
        match self {
            Self::Dragging(selection) | Self::Selected(selection) => Some(selection),
            Self::None => None,
        }
    }
}

/// Cursor and text editing keys shared by every free-text composer.
///
/// Owners check their own chords (submit, cancel, configurable bindings)
/// first, then map the rest through [`ComposerEditKey::from_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComposerEditKey {
    WordBackspace,
    Backspace,
    Delete,
    WordLeft,
    WordRight,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Newline,
    Char(char),
}

impl ComposerEditKey {
    /// Alt acts on words; other arrows, Home, End, Backspace, and Delete ignore
    /// modifiers; Shift+Enter inserts a newline; a char inserts unless Ctrl or
    /// Alt is held.
    pub(super) fn from_key(key: KeyEvent) -> Option<Self> {
        let edit = match (key.modifiers, key.code) {
            (KeyModifiers::ALT, KeyCode::Backspace) => Self::WordBackspace,
            (_, KeyCode::Backspace) => Self::Backspace,
            (_, KeyCode::Delete) => Self::Delete,
            (KeyModifiers::ALT, KeyCode::Left) => Self::WordLeft,
            (KeyModifiers::ALT, KeyCode::Right) => Self::WordRight,
            (_, KeyCode::Left) => Self::Left,
            (_, KeyCode::Right) => Self::Right,
            (_, KeyCode::Up) => Self::Up,
            (_, KeyCode::Down) => Self::Down,
            (_, KeyCode::Home) => Self::Home,
            (_, KeyCode::End) => Self::End,
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => Self::Newline,
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Self::Char(ch)
            }
            _ => return None,
        };
        Some(edit)
    }
}

/// What [`ComposerBuffer::apply_edit`] did, so owners can run their side effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditOutcome {
    /// Text changed (possibly by an empty replacement).
    Edited,
    /// Text is unchanged; the caret or selection may have moved.
    TextUnchanged,
    /// Up on the first row or Down on the last row. The caret has not moved:
    /// the owner may recall history, else call [`ComposerBuffer::move_vertically`].
    VerticalEdge(HistoryDirection),
}

/// Composer text with a char-indexed caret, mouse selection, and atomic
/// collapsed-paste markers. See the module docs for what owners layer on top.
#[derive(Clone, Debug, Default)]
pub(super) struct ComposerBuffer {
    text: String,
    cursor: usize,
    selection: ComposerSelectionState,
    /// First painted visual row; retained by the paint path only.
    view_start: usize,
    /// Wrapped text width at the last paint; vertical moves follow it.
    /// `None` until the first paint, when rows break only at newlines.
    wrap_width: Option<usize>,
    paste_segments: Vec<PasteSegment>,
}

impl ComposerBuffer {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(super) fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
    }

    pub(super) fn view_start(&self) -> usize {
        self.view_start
    }

    /// Retain the painted row window and wrap width. Only the paint path
    /// calls this, so speculative layouts never move the window.
    pub(super) fn retain_paint(&mut self, view_start: usize, wrap_width: usize) {
        self.view_start = view_start;
        self.wrap_width = Some(wrap_width.max(1));
    }

    pub(super) fn reset_view_start(&mut self) {
        self.view_start = 0;
    }

    fn wrap_width(&self) -> usize {
        self.wrap_width.unwrap_or(usize::MAX)
    }

    pub(super) fn paste_segments(&self) -> &[PasteSegment] {
        &self.paste_segments
    }

    pub(super) fn set_paste_segments(&mut self, segments: Vec<PasteSegment>) {
        self.paste_segments = segments;
    }

    /// Text with collapsed paste markers replaced by their content.
    pub(super) fn expanded_text(&self) -> String {
        super::paste_burst::expand_paste_segments(&self.text, &self.paste_segments)
    }

    /// Replace the text, keeping the caret and paste segments as they are.
    pub(super) fn set_text(&mut self, text: String) {
        self.text = text;
        self.selection = ComposerSelectionState::None;
        self.view_start = 0;
    }

    pub(super) fn set_text_and_cursor(&mut self, text: String, cursor: usize) {
        self.set_text(text);
        self.cursor = cursor;
    }

    /// Load a whole draft with the caret at its end.
    pub(super) fn replace_all(&mut self, text: String, paste_segments: Vec<PasteSegment>) {
        let cursor = text.chars().count();
        self.set_text_and_cursor(text, cursor);
        self.paste_segments = paste_segments;
    }

    pub(super) fn clear(&mut self) {
        self.replace_all(String::new(), Vec::new());
    }

    /// Take the expanded text and leave the buffer empty.
    pub(super) fn take_expanded(&mut self) -> String {
        let text = self.expanded_text();
        self.clear();
        text
    }

    // Selection.

    pub(super) fn selection_focus(&self) -> Option<usize> {
        self.selection.value().map(ComposerSelection::focus)
    }

    pub(super) fn selection_pointer_origin(&self) -> Option<usize> {
        self.selection
            .value()
            .map(ComposerSelection::pointer_origin)
    }

    pub(super) fn selection_dragging(&self) -> bool {
        matches!(self.selection, ComposerSelectionState::Dragging(_))
    }

    /// Highlight/edit range when the selection spans at least one character.
    pub(super) fn selection_range(&self) -> Option<std::ops::Range<usize>> {
        self.selection
            .value()
            .and_then(ComposerSelection::edit_range)
    }

    pub(super) fn begin_selection(&mut self, position: usize) {
        self.selection = ComposerSelectionState::Dragging(ComposerSelection::characters(position));
    }

    /// Select an existing character range (for example double-click word select).
    ///
    /// Keeps the primary-button drag active so the user can extend the range.
    pub(super) fn select_range(&mut self, start: usize, end: usize) {
        if start == end {
            self.clear_selection();
            return;
        }
        self.selection = ComposerSelectionState::Dragging(ComposerSelection::range(start, end));
    }

    pub(super) fn update_selection(&mut self, position: usize) {
        if let ComposerSelectionState::Dragging(selection) = &mut self.selection {
            selection.update(position);
        }
    }

    /// Keep a non-empty selection after mouse release; drop a collapsed click.
    pub(super) fn finalize_selection(&mut self) {
        self.selection = match self.selection {
            ComposerSelectionState::Dragging(selection) if selection.edit_range().is_some() => {
                ComposerSelectionState::Selected(selection)
            }
            ComposerSelectionState::Selected(selection) => {
                ComposerSelectionState::Selected(selection)
            }
            ComposerSelectionState::Dragging(_) | ComposerSelectionState::None => {
                ComposerSelectionState::None
            }
        };
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection = ComposerSelectionState::None;
    }

    /// Take a non-empty selection range and clear selection state.
    pub(super) fn take_selection_range(&mut self) -> Option<std::ops::Range<usize>> {
        let range = self.selection_range();
        self.clear_selection();
        range
    }

    // Pointer.

    /// Snap a pointer caret into an atomic collapsed-paste marker.
    pub(super) fn caret_index(&self, index: usize) -> usize {
        self.paste_segments
            .iter()
            .find(|segment| segment.start < index && index < segment.end())
            .map_or(index, |segment| segment.start)
    }

    /// Expand a drag endpoint to the nearest edge of an atomic paste marker.
    pub(super) fn selection_focus_index(&self, index: usize) -> usize {
        let Some(origin) = self.selection_pointer_origin() else {
            return index;
        };
        self.paste_segments
            .iter()
            .find(|segment| segment.start < index && index < segment.end())
            .map_or(index, |segment| {
                if index < origin {
                    segment.start
                } else {
                    segment.end()
                }
            })
    }

    /// Primary press on caret index `index` (already snapped with
    /// [`Self::caret_index`]). A double click selects the word or paste
    /// marker under it; a single click starts a drag selection.
    pub(super) fn pointer_press(&mut self, index: usize, double_click: bool) {
        if double_click {
            let range = self
                .paste_segments
                .iter()
                .find(|segment| segment.start <= index && index < segment.end())
                .map(|segment| segment.start..segment.end())
                .unwrap_or_else(|| word_range_at(&self.text, index));
            self.select_range(range.start, range.end);
            self.cursor = range.end;
        } else {
            self.begin_selection(index);
            self.cursor = index;
        }
    }

    /// Extend an active drag selection to raw char index `index`.
    pub(super) fn pointer_drag(&mut self, index: usize) {
        let index = self.selection_focus_index(index);
        self.update_selection(index);
        self.cursor = index;
    }

    /// End a drag selection, extending it to `index` when the release landed
    /// on text.
    pub(super) fn pointer_release(&mut self, index: Option<usize>) {
        if let Some(index) = index {
            self.pointer_drag(index);
        }
        let focus = self.selection_focus();
        self.finalize_selection();
        if let Some(focus) = focus {
            self.cursor = focus;
        }
    }

    // Layout.

    /// Map a wrapped row/column at `width` to a character index.
    pub(super) fn char_index_at(&self, width: usize, row: usize, column: usize) -> usize {
        input_char_index_at_position(&self.text, width, row, column)
    }

    /// Wrapped rows at `width` with selection or focused-marker highlight.
    pub(super) fn frame(&self, width: usize) -> InputFrame {
        let focused_paste = self
            .focused_paste_segment()
            .map(|segment| segment.start..segment.end());
        let highlighted = self.selection_range().or(focused_paste);
        input_frame(&self.text, self.cursor, width, highlighted)
    }

    pub(super) fn focused_paste_segment(&self) -> Option<&PasteSegment> {
        self.paste_segments
            .iter()
            .find(|segment| segment.start == self.cursor)
    }

    // Edits.

    /// Apply a shared edit key. Vertical moves follow the painted wrap width.
    pub(super) fn apply_edit(&mut self, edit: ComposerEditKey) -> EditOutcome {
        let edited = match edit {
            ComposerEditKey::WordBackspace => {
                self.delete_word_before_cursor();
                true
            }
            ComposerEditKey::Backspace => self.backspace(),
            ComposerEditKey::Delete => self.delete(),
            ComposerEditKey::WordLeft => {
                self.move_to_previous_word();
                false
            }
            ComposerEditKey::WordRight => {
                self.move_to_next_word();
                false
            }
            ComposerEditKey::Left => {
                self.move_left();
                false
            }
            ComposerEditKey::Right => {
                self.move_right();
                false
            }
            ComposerEditKey::Up => return self.vertical_edit(HistoryDirection::Previous),
            ComposerEditKey::Down => return self.vertical_edit(HistoryDirection::Next),
            ComposerEditKey::Home => {
                self.clear_selection();
                self.cursor = 0;
                false
            }
            ComposerEditKey::End => {
                self.clear_selection();
                self.cursor = self.char_len();
                false
            }
            ComposerEditKey::Newline => {
                self.insert_text("\n");
                true
            }
            ComposerEditKey::Char(ch) => {
                self.insert_text(ch.encode_utf8(&mut [0; 4]));
                true
            }
        };
        if edited {
            EditOutcome::Edited
        } else {
            EditOutcome::TextUnchanged
        }
    }

    fn vertical_edit(&mut self, direction: HistoryDirection) -> EditOutcome {
        self.clear_selection();
        let visual_lines = editable_input_visual_lines(&self.text, self.wrap_width());
        let caret = visual_caret_position(&visual_lines, &self.text, self.cursor);
        let at_edge = match direction {
            HistoryDirection::Previous => caret.y == 0,
            HistoryDirection::Next => caret.y as usize + 1 >= visual_lines.len(),
        };
        if at_edge {
            return EditOutcome::VerticalEdge(direction);
        }
        self.move_vertically(direction);
        EditOutcome::TextUnchanged
    }

    /// Move one wrapped row, keeping the display column. Past the last row
    /// the caret lands at the end of the text.
    pub(super) fn move_vertically(&mut self, direction: HistoryDirection) {
        let visual_lines = editable_input_visual_lines(&self.text, self.wrap_width());
        let caret = visual_caret_position(&visual_lines, &self.text, self.cursor);
        let target_row = match direction {
            HistoryDirection::Previous => caret.y.saturating_sub(1) as usize,
            HistoryDirection::Next => caret.y as usize + 1,
        };
        self.cursor = input_cursor_index_on_visual_line(
            &self.text,
            &visual_lines,
            target_row,
            caret.x as usize,
        );
        self.focus_paste_segment_at_cursor();
    }

    pub(super) fn move_left(&mut self) {
        if let Some(range) = self.take_selection_range() {
            self.cursor = range.start;
            return;
        }
        let cursor = self.cursor;
        self.cursor = self
            .paste_segments
            .iter()
            .find(|segment| segment.start < cursor && cursor <= segment.end())
            .map_or(cursor.saturating_sub(1), |segment| segment.start);
    }

    pub(super) fn move_right(&mut self) {
        if let Some(range) = self.take_selection_range() {
            self.cursor = range.end;
            return;
        }
        let cursor = self.cursor;
        self.cursor = self
            .paste_segments
            .iter()
            .find(|segment| segment.start <= cursor && cursor < segment.end())
            .map_or((cursor + 1).min(self.char_len()), PasteSegment::end);
    }

    pub(super) fn move_to_previous_word(&mut self) {
        if let Some(range) = self.take_selection_range() {
            self.cursor = range.start;
            return;
        }
        self.cursor = previous_word_boundary(&self.text, self.cursor);
    }

    pub(super) fn move_to_next_word(&mut self) {
        if let Some(range) = self.take_selection_range() {
            self.cursor = range.end;
            return;
        }
        self.cursor = next_word_boundary(&self.text, self.cursor);
    }

    pub(super) fn focus_paste_segment_at_cursor(&mut self) {
        let cursor = self.cursor;
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start < cursor && cursor < segment.end())
        {
            self.cursor = segment.start;
        }
    }

    /// Replace chars `start..end` with `text`, widening the range to swallow
    /// any paste marker it touches. `paste_content` turns the inserted text
    /// into a collapsed marker for that content.
    pub(super) fn replace_range(
        &mut self,
        start: usize,
        end: usize,
        text: &str,
        paste_content: Option<String>,
    ) {
        self.clear_selection();
        let range = self.normalize_edit_range(start..end);
        let inserted_len = text.chars().count();
        self.adjust_paste_segments_for_edit(range.start, range.len(), inserted_len);
        let start_byte = self.byte_index(range.start);
        let end_byte = self.byte_index(range.end);
        self.text.replace_range(start_byte..end_byte, text);
        self.cursor = range.start + inserted_len;
        if let Some(content) = paste_content {
            self.paste_segments.push(PasteSegment {
                start: range.start,
                marker_len: inserted_len,
                content,
            });
            self.paste_segments.sort_by_key(|segment| segment.start);
        }
    }

    /// Replace a non-empty selection with `text`; false when nothing is selected.
    pub(super) fn replace_selection(&mut self, text: &str) -> bool {
        let Some(range) = self.take_selection_range() else {
            return false;
        };
        self.replace_range(range.start, range.end, text, None);
        true
    }

    /// Insert `text` over the selection or at the caret.
    pub(super) fn insert_text(&mut self, text: &str) {
        if !self.replace_selection(text) {
            self.replace_range(self.cursor, self.cursor, text, None);
        }
    }

    /// Insert a collapsed marker for `paste` that expands to `content`.
    pub(super) fn insert_collapsed_paste(&mut self, paste: &CollapsedPaste, content: &str) {
        let range = self
            .take_selection_range()
            .unwrap_or(self.cursor..self.cursor);
        self.replace_range(
            range.start,
            range.end,
            &paste.marker(),
            Some(content.to_owned()),
        );
    }

    /// Remove the selection, the marker before the caret, or one char.
    /// Returns false when the caret is at the start with nothing selected.
    pub(super) fn backspace(&mut self) -> bool {
        if self.replace_selection("") {
            return true;
        }
        let cursor = self.cursor;
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start < cursor && cursor <= segment.end())
            .cloned()
        {
            self.replace_range(segment.start, segment.end(), "", None);
            return true;
        }
        if cursor == 0 {
            return false;
        }
        self.replace_range(cursor - 1, cursor, "", None);
        true
    }

    /// Remove the selection, the marker after the caret, or one char.
    /// Returns false when the caret is at the end with nothing selected.
    pub(super) fn delete(&mut self) -> bool {
        if self.replace_selection("") {
            return true;
        }
        let cursor = self.cursor;
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start <= cursor && cursor < segment.end())
            .cloned()
        {
            self.replace_range(segment.start, segment.end(), "", None);
            return true;
        }
        if cursor >= self.char_len() {
            return false;
        }
        self.replace_range(cursor, cursor + 1, "", None);
        true
    }

    pub(super) fn delete_word_before_cursor(&mut self) {
        if self.replace_selection("") {
            return;
        }
        let start = previous_word_boundary(&self.text, self.cursor);
        self.replace_range(start, self.cursor, "", None);
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(index, _)| index)
    }

    /// Expand an edit to consume any collapsed paste marker it intersects.
    fn normalize_edit_range(&self, mut range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let char_len = self.char_len();
        range.start = range.start.min(char_len);
        range.end = range.end.max(range.start).min(char_len);
        for segment in &self.paste_segments {
            let intersects = range.start < segment.end() && range.end > segment.start;
            let caret_inside =
                range.is_empty() && segment.start < range.start && range.start < segment.end();
            if intersects || caret_inside {
                range.start = range.start.min(segment.start);
                range.end = range.end.max(segment.end());
            }
        }
        range
    }

    fn adjust_paste_segments_for_edit(
        &mut self,
        start: usize,
        deleted_len: usize,
        inserted_len: usize,
    ) {
        let end = start + deleted_len;
        let shift = inserted_len as isize - deleted_len as isize;
        self.paste_segments.retain_mut(|segment| {
            if start < segment.end() && end > segment.start {
                return false;
            }
            if start <= segment.start {
                segment.start = segment.start.saturating_add_signed(shift);
            }
            true
        });
    }
}

#[cfg(test)]
#[path = "composer_buffer_tests.rs"]
mod tests;
