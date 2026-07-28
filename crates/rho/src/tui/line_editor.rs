//! Shared single-line text buffer used by overlay text inputs.

use super::picker::UiPicker;

/// Cursor-aware single-line editor with an optional picker to restore on cancel/save.
#[derive(Clone, Debug)]
pub(super) struct LineEditor {
    pub(super) value: String,
    pub(super) cursor: usize,
    return_picker: Option<Box<UiPicker>>,
}

impl LineEditor {
    pub(super) fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self {
            value,
            cursor,
            return_picker: None,
        }
    }

    pub(super) fn with_return_picker(mut self, picker: UiPicker) -> Self {
        self.return_picker = Some(Box::new(picker));
        self
    }

    pub(super) fn take_return_picker(&mut self) -> Option<UiPicker> {
        self.return_picker.take().map(|picker| *picker)
    }

    fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        if ch == '\n' || ch == '\r' {
            return;
        }
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        let sanitized = text.replace(['\n', '\r'], "");
        let byte_index = self.byte_index(self.cursor);
        self.value.insert_str(byte_index, &sanitized);
        self.cursor += sanitized.chars().count();
    }

    pub(super) fn move_cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn move_cursor_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_len());
    }

    pub(super) fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_cursor_end(&mut self) {
        self.cursor = self.char_len();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }
}
