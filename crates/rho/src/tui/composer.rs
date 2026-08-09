use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    commands,
    composer_layout::{content_width, prompt_width},
    paste_burst::{normalize_paste, paste_marker_for, previous_word_boundary},
    render::{
        editable_input_visual_lines, input_char_index_at_position,
        input_cursor_index_on_visual_line, input_cursor_position,
    },
    App, CommandInvocation, ComposerAttachment, ComposerMode, HistoryDirection, InputDraft,
    InputSubmissionMode, PasteBurstEnter, PasteBurstKey, PasteSegment,
};

impl App {
    pub(super) fn flush_due_paste_burst(&mut self) -> bool {
        if self.input_ui.paste_burst().is_due(Instant::now()) {
            self.flush_pending_paste_burst();
            true
        } else {
            false
        }
    }

    pub(super) fn flush_pending_paste_burst(&mut self) {
        let Some(text) = self.input_ui.paste_burst_mut().take_pending() else {
            return;
        };
        let text = normalize_paste(&text);
        self.insert_external_paste(&text);
    }

    pub(super) fn handle_paste_burst_key(&mut self, key: KeyEvent) -> bool {
        self.handle_paste_burst_key_at(key, Instant::now())
    }

    pub(super) fn handle_paste_burst_key_at(&mut self, key: KeyEvent, now: Instant) -> bool {
        let Some(burst_key) = self.paste_burst_key(key) else {
            self.flush_pending_paste_burst();
            return false;
        };

        match burst_key {
            PasteBurstKey::Char(ch) => {
                if !self.input_ui.paste_burst().can_continue(now) {
                    self.flush_pending_paste_burst();
                }
                self.input_ui.paste_burst_mut().push_plain_char(ch, now);
                self.ctrl_c_streak = 0;
                true
            }
            PasteBurstKey::Enter => {
                match self.input_ui.paste_burst_mut().push_enter_if_paste(now) {
                    PasteBurstEnter::Buffered => {
                        self.ctrl_c_streak = 0;
                        true
                    }
                    PasteBurstEnter::InsertNewline => {
                        self.insert_paste_burst_newline();
                        self.ctrl_c_streak = 0;
                        true
                    }
                    PasteBurstEnter::NotPaste => {
                        self.flush_pending_paste_burst();
                        false
                    }
                }
            }
        }
    }

    fn insert_paste_burst_newline(&mut self) {
        match self.input_ui.composer_mut() {
            ComposerMode::Input => self.insert_input_char('\n'),
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.insert_char('\n');
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::TextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::InteractivePending(_) => {}
        }
    }

    fn paste_burst_key(&self, key: KeyEvent) -> Option<PasteBurstKey> {
        match (key.modifiers, key.code) {
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.composer_accepts_paste_burst_char(ch) =>
            {
                Some(PasteBurstKey::Char(ch))
            }
            (KeyModifiers::NONE, KeyCode::Enter) if self.composer_accepts_paste_burst_enter() => {
                Some(PasteBurstKey::Enter)
            }
            _ => None,
        }
    }

    fn composer_accepts_paste_burst_char(&self, ch: char) -> bool {
        match self.input_ui.composer() {
            ComposerMode::Input => true,
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.accepts_paste_burst_char(ch)
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::TextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::InteractivePending(_) => false,
        }
    }

    fn composer_accepts_paste_burst_enter(&self) -> bool {
        match self.input_ui.composer() {
            ComposerMode::Input => true,
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.active_text_entry_active()
                    || (self.input_ui.paste_burst().has_pending()
                        && questionnaire.accepts_pending_paste_burst_enter())
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::TextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::InteractivePending(_) => false,
        }
    }

    pub(super) fn input_char_len(&self) -> usize {
        self.input_ui.char_len()
    }

    fn input_byte_index(&self, char_index: usize) -> usize {
        self.input_ui
            .text()
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.input_ui.text().len())
    }

    pub(super) fn reset_input_history_navigation(&mut self) {
        self.input_ui.reset_history_navigation();
    }

    pub(super) fn push_input_history(&mut self, prompt: &str) {
        self.reset_input_history_navigation();
        if prompt.is_empty() {
            return;
        }
        self.input_ui.push_history_if_new(prompt);
    }

    fn recall_input_history(&mut self, direction: HistoryDirection) -> bool {
        if self.input_ui.history().is_empty() {
            return false;
        }

        let next_cursor = match (direction, self.input_ui.history_cursor()) {
            (HistoryDirection::Previous, None) => {
                self.input_ui.set_history_draft(Some(InputDraft {
                    input: self.input_ui.text().to_string(),
                    paste_segments: self.input_ui.paste_segments().to_vec(),
                    submission_mode: self.input_ui.submission_mode(),
                    shell_mode: self.input_ui.shell_mode(),
                }));
                self.input_ui.history().len() - 1
            }
            (HistoryDirection::Previous, Some(0)) => 0,
            (HistoryDirection::Previous, Some(cursor)) => cursor - 1,
            (HistoryDirection::Next, None) => return false,
            (HistoryDirection::Next, Some(cursor))
                if cursor + 1 < self.input_ui.history().len() =>
            {
                cursor + 1
            }
            (HistoryDirection::Next, Some(_)) => {
                let draft = self.input_ui.take_history_draft().unwrap_or(InputDraft {
                    input: String::new(),
                    paste_segments: Vec::new(),
                    submission_mode: InputSubmissionMode::ParseCommands,
                    shell_mode: None,
                });
                self.input_ui.apply_input_draft(draft);
                self.input_ui.set_history_cursor(None);
                self.input_changed();
                return true;
            }
        };

        self.apply_composer_text(
            self.input_ui.history()[next_cursor].clone(),
            Vec::new(),
            InputSubmissionMode::ParseCommands,
        );
        self.input_ui.set_history_cursor(Some(next_cursor));
        true
    }

    pub(super) fn recall_input_history_or_move_cursor(
        &mut self,
        direction: HistoryDirection,
        terminal_width: usize,
    ) {
        self.input_ui.clear_selection();
        let content_width = content_width(terminal_width);
        let visual_lines = editable_input_visual_lines(self.input_ui.text(), content_width);
        let cursor_position =
            input_cursor_position(self.input_ui.text(), self.input_ui.cursor(), content_width);
        let can_recall = match direction {
            HistoryDirection::Previous => cursor_position.y == 0,
            HistoryDirection::Next => cursor_position.y as usize + 1 >= visual_lines.len(),
        };

        if can_recall && self.recall_input_history(direction) {
            return;
        }

        let target_row = match direction {
            HistoryDirection::Previous => cursor_position.y.saturating_sub(1) as usize,
            HistoryDirection::Next => cursor_position.y as usize + 1,
        };
        self.input_ui.set_cursor(input_cursor_index_on_visual_line(
            self.input_ui.text(),
            &visual_lines,
            target_row,
            cursor_position.x as usize,
        ));
        self.focus_paste_segment_at_cursor();
    }

    pub(super) fn move_input_cursor_left(&mut self) {
        if let Some(range) = self.input_ui.take_selection_range() {
            self.input_ui.set_cursor(range.start);
            return;
        }
        if let Some(segment) = self.input_ui.paste_segments().iter().find(|segment| {
            segment.start < self.input_ui.cursor() && self.input_ui.cursor() <= segment.end()
        }) {
            self.input_ui.set_cursor(segment.start);
        } else {
            self.input_ui
                .set_cursor(self.input_ui.cursor().saturating_sub(1));
        }
    }

    pub(super) fn move_input_cursor_right(&mut self) {
        if let Some(range) = self.input_ui.take_selection_range() {
            self.input_ui.set_cursor(range.end);
            return;
        }
        if let Some(segment) = self.input_ui.paste_segments().iter().find(|segment| {
            segment.start <= self.input_ui.cursor() && self.input_ui.cursor() < segment.end()
        }) {
            self.input_ui.set_cursor(segment.end());
        } else {
            self.input_ui
                .set_cursor((self.input_ui.cursor() + 1).min(self.input_char_len()));
        }
    }

    pub(super) fn focus_paste_segment_at_cursor(&mut self) {
        if let Some(segment) = self.input_ui.paste_segments().iter().find(|segment| {
            segment.start < self.input_ui.cursor() && self.input_ui.cursor() < segment.end()
        }) {
            self.input_ui.set_cursor(segment.start);
        }
    }

    pub(super) fn move_input_cursor_to_previous_word(&mut self) {
        if let Some(range) = self.input_ui.take_selection_range() {
            self.input_ui.set_cursor(range.start);
            return;
        }
        self.input_ui.set_cursor(previous_word_boundary(
            self.input_ui.text(),
            self.input_ui.cursor(),
        ));
    }

    pub(super) fn move_input_cursor_to_next_word(&mut self) {
        if let Some(range) = self.input_ui.take_selection_range() {
            self.input_ui.set_cursor(range.end);
            return;
        }
        self.input_ui
            .set_cursor(super::paste_burst::next_word_boundary(
                self.input_ui.text(),
                self.input_ui.cursor(),
            ));
    }

    pub(super) fn focused_paste_segment(&self) -> Option<&PasteSegment> {
        self.input_ui
            .paste_segments()
            .iter()
            .find(|segment| segment.start == self.input_ui.cursor())
    }

    pub(super) fn replace_input_range(&mut self, start: usize, end: usize, text: &str) {
        self.replace_input_range_with_paste_content(start, end, text, None);
    }

    fn replace_input_range_with_paste_content(
        &mut self,
        start: usize,
        end: usize,
        text: &str,
        paste_content: Option<String>,
    ) {
        self.reset_input_history_navigation();
        self.input_ui.clear_selection();
        let range = self.normalize_input_edit_range(start..end);
        let inserted_len = text.chars().count();
        self.adjust_paste_segments_for_edit(range.start, range.len(), inserted_len);
        let start_byte = self.input_byte_index(range.start);
        let end_byte = self.input_byte_index(range.end);
        self.input_ui
            .with_text_mut(|value| value.replace_range(start_byte..end_byte, text));
        self.input_ui.set_cursor(range.start + inserted_len);
        if let Some(content) = paste_content {
            self.input_ui.paste_segments_mut().push(PasteSegment {
                start: range.start,
                marker_len: inserted_len,
                content,
            });
            self.input_ui
                .paste_segments_mut()
                .sort_by_key(|segment| segment.start);
        }
        self.input_changed();
    }

    /// Expand an edit to consume any collapsed paste marker it intersects.
    fn normalize_input_edit_range(
        &self,
        mut range: std::ops::Range<usize>,
    ) -> std::ops::Range<usize> {
        let char_len = self.input_char_len();
        range.start = range.start.min(char_len);
        range.end = range.end.max(range.start).min(char_len);
        for segment in self.input_ui.paste_segments() {
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

    fn replace_input_selection(&mut self, text: &str) -> bool {
        let Some(range) = self.input_ui.take_selection_range() else {
            return false;
        };
        self.replace_input_range(range.start, range.end, text);
        true
    }

    pub(super) fn insert_input_char(&mut self, ch: char) {
        if self.replace_input_selection(&ch.to_string()) {
            return;
        }
        if ch == '!' && self.try_enter_shell_mode_from_bang() {
            return;
        }
        let cursor = self.input_ui.cursor();
        self.replace_input_range(cursor, cursor, &ch.to_string());
    }

    /// Insert plain composer text through the char path so rules like shell-mode
    /// bang handling stay single-sourced. Paste-burst flushes land here; collapsed
    /// paste markers use [`Self::insert_pasted_input_text`] instead.
    pub(super) fn insert_input_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.replace_input_selection(text) {
            return;
        }
        for ch in text.chars() {
            self.insert_input_char(ch);
        }
    }

    pub(super) fn insert_pasted_input_text(&mut self, text: &str) {
        let Some(marker) = paste_marker_for(text) else {
            self.insert_input_text(text);
            return;
        };
        self.insert_input_text_with_paste_content(&marker, Some(text.to_string()));
    }

    fn insert_input_text_with_paste_content(&mut self, text: &str, paste_content: Option<String>) {
        let range = self
            .input_ui
            .take_selection_range()
            .unwrap_or_else(|| self.input_ui.cursor()..self.input_ui.cursor());
        self.replace_input_range_with_paste_content(range.start, range.end, text, paste_content);
    }

    pub(super) fn expanded_input(&self) -> String {
        self.input_ui.expanded_text()
    }

    fn adjust_paste_segments_for_edit(
        &mut self,
        start: usize,
        deleted_len: usize,
        inserted_len: usize,
    ) {
        let end = start + deleted_len;
        let shift = inserted_len as isize - deleted_len as isize;
        self.input_ui.paste_segments_mut().retain_mut(|segment| {
            if start < segment.end() && end > segment.start {
                return false;
            }
            if start <= segment.start {
                segment.start = segment.start.saturating_add_signed(shift);
            }
            true
        });
    }

    pub(super) fn backspace_input(&mut self) {
        if self.replace_input_selection("") {
            return;
        }
        if let Some(segment) = self
            .input_ui
            .paste_segments()
            .iter()
            .find(|segment| {
                segment.start < self.input_ui.cursor() && self.input_ui.cursor() <= segment.end()
            })
            .cloned()
        {
            self.replace_input_range(segment.start, segment.end(), "");
            return;
        }
        if self.input_ui.cursor() == 0 {
            if self.input_ui.text().is_empty() {
                match self.input_ui.pop_attachment() {
                    Some(ComposerAttachment::Pending { id, .. }) => {
                        self.cancel_pending_attachment(id);
                        let pending_count = self.input_ui.pending_attachment_count();
                        self.set_status(if pending_count == 0 {
                            "document extraction cancelled".to_string()
                        } else {
                            format!("extracting files: {pending_count}")
                        });
                    }
                    Some(ComposerAttachment::Ready(_)) => {
                        self.set_status(format!(
                            "attachments: {}",
                            self.input_ui.attachments().len()
                        ));
                    }
                    None => {}
                }
            }
            return;
        }
        let edit_start = self.input_ui.cursor() - 1;
        self.replace_input_range(edit_start, self.input_ui.cursor(), "");
    }

    pub(super) fn delete_input(&mut self) {
        if self.replace_input_selection("") {
            return;
        }
        if let Some(segment) = self
            .input_ui
            .paste_segments()
            .iter()
            .find(|segment| {
                segment.start <= self.input_ui.cursor() && self.input_ui.cursor() < segment.end()
            })
            .cloned()
        {
            self.replace_input_range(segment.start, segment.end(), "");
            return;
        }
        if self.input_ui.cursor() >= self.input_char_len() {
            return;
        }
        self.replace_input_range(self.input_ui.cursor(), self.input_ui.cursor() + 1, "");
    }

    pub(super) fn delete_word_before_cursor(&mut self) {
        if self.replace_input_selection("") {
            return;
        }
        let start_cursor = previous_word_boundary(self.input_ui.text(), self.input_ui.cursor());
        self.replace_input_range(start_cursor, self.input_ui.cursor(), "");
    }

    /// Snap a pointer caret into an atomic collapsed-paste marker.
    pub(super) fn composer_caret_index(&self, index: usize) -> usize {
        self.input_ui
            .paste_segments()
            .iter()
            .find(|segment| segment.start < index && index < segment.end())
            .map_or(index, |segment| segment.start)
    }

    /// Expand a drag endpoint to the nearest edge of an atomic paste marker.
    pub(super) fn composer_selection_focus(&self, index: usize) -> usize {
        let Some(origin) = self.input_ui.selection_pointer_origin() else {
            return index;
        };
        self.input_ui
            .paste_segments()
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

    /// Hit-test the free-text composer for pointer placement and selection.
    pub(super) fn composer_text_char_index_at(
        &self,
        layout: &super::screen_layout::ScreenLayout,
        column: u16,
        row: u16,
        clamp_to_composer: bool,
    ) -> Option<usize> {
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return None;
        }
        let composer = layout.composer;
        if composer.width == 0 || composer.height == 0 {
            return None;
        }
        let inside = composer.contains(ratatui::layout::Position { x: column, y: row });
        if !inside && !clamp_to_composer {
            return None;
        }
        let column = if clamp_to_composer {
            column.clamp(
                composer.x,
                composer.x.saturating_add(composer.width.saturating_sub(1)),
            )
        } else {
            column
        };
        let row = if clamp_to_composer {
            row.clamp(
                composer.y,
                composer.y.saturating_add(composer.height.saturating_sub(1)),
            )
        } else if !inside {
            return None;
        } else {
            row
        };

        let attachment_rows = self.input_ui.attachments().len();
        let visible_row = row.saturating_sub(composer.y) as usize;
        let absolute_row = layout.composer_start.saturating_add(visible_row);
        if absolute_row < attachment_rows {
            // Attachment labels are not part of the text buffer.
            return None;
        }
        let text_row = absolute_row.saturating_sub(attachment_rows);
        let width = composer.width as usize;
        let content_column =
            (column.saturating_sub(composer.x) as usize).saturating_sub(prompt_width());
        Some(input_char_index_at_position(
            self.input_ui.text(),
            content_width(width),
            text_row,
            content_column,
        ))
    }

    /// True when the pointer is over the free-text composer rect (including labels).
    pub(super) fn pointer_in_composer(
        &self,
        layout: &super::screen_layout::ScreenLayout,
        column: u16,
        row: u16,
    ) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::Input)
            && layout
                .composer
                .contains(ratatui::layout::Position { x: column, y: row })
    }

    pub(super) fn replace_composer_from_editor(&mut self, text: String) {
        self.reset_input_history_navigation();
        let cursor = text.chars().count();
        self.input_ui.set_text_and_cursor(text, cursor);
        self.input_ui.clear_paste_segments();
        self.input_ui.clear_paste_burst();
        self.input_changed();
    }

    pub(super) fn input_changed(&mut self) {
        self.input_ui.set_command_palette_dismissed(false);
        self.input_ui.set_file_palette_dismissed(false);
        self.clamp_command_selection();
        self.clamp_file_selection();
    }

    pub(super) fn parse_input_command(
        &mut self,
    ) -> Result<Option<CommandInvocation>, commands::CommandParseError> {
        match self.input_ui.take_submission_mode() {
            InputSubmissionMode::ParseCommands => {
                let result = commands::parse_command(self.input_ui.text());
                if matches!(result, Ok(Some(_))) {
                    let command = self.input_ui.text().trim_end().to_string();
                    self.push_input_history(&command);
                }
                result
            }
            InputSubmissionMode::Prompt => Ok(None),
        }
    }

    pub(super) fn command_palette_visible(&self) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::Input)
            && self.input_ui.shell_mode().is_none()
            && !self.input_ui.command_palette_dismissed()
            && (self.cursor_in_command_token()
                || !commands::argument_choices(self.input_ui.text(), self.input_ui.cursor())
                    .is_empty()
                || !self.mcp_argument_choices().is_empty())
            && !self.command_matches().is_empty()
    }

    fn cursor_in_command_token(&self) -> bool {
        if !self.input_ui.text().starts_with('/') {
            return false;
        }

        let token_len = self
            .input_ui
            .text()
            .chars()
            .position(char::is_whitespace)
            .unwrap_or_else(|| self.input_char_len());
        self.input_ui.cursor() <= token_len
    }

    pub(super) fn clamp_command_selection(&mut self) {
        // What the palette is answering: a command prefix while the cursor is
        // still in the command token, otherwise the argument value it has moved
        // on to. Either way a change starts the selection over, so a row picked
        // for one question is never left highlighted for the next.
        let in_command_token = self.cursor_in_command_token();
        let prefix = if in_command_token {
            commands::command_prefix(self.input_ui.text()).map(str::to_ascii_lowercase)
        } else {
            self.mcp_argument_cursor()
                .map(|cursor| cursor.palette_identity())
        };
        if self.input_ui.command_prefix() != prefix.as_deref() {
            self.input_ui.set_command_prefix(prefix);
            self.input_ui.set_command_selection(0);
        }
        if in_command_token && self.input_ui.command_prefix().is_some() {
            self.refresh_skill_match_cache();
        }

        let match_count = self.command_matches().len();
        if match_count == 0 {
            self.input_ui.set_command_selection(0);
        } else if self.input_ui.command_selection() >= match_count {
            self.input_ui.set_command_selection(match_count - 1);
        }
    }

    pub(super) fn insert_paste(&mut self, text: &str) {
        match self.input_ui.composer_mut() {
            ComposerMode::Input => self.insert_pasted_input_text(text),
            ComposerMode::SecretInput(secret) => secret.insert_text(text),
            ComposerMode::ConfigNumberInput(input) => input.insert_text(text),
            ComposerMode::TextInput(input) => input.editor.insert_text(text),
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.insert_text(text);
            }
            ComposerMode::Approval(_)
            | ComposerMode::Picker(_)
            | ComposerMode::InteractivePending(_)
            | ComposerMode::InlineChoice(_) => {}
        }
    }

    pub(super) fn insert_external_paste(&mut self, text: &str) {
        let is_command = matches!(commands::parse_command(text), Ok(Some(_)));
        if is_command || !self.start_pasted_media_path(text) {
            self.insert_paste(text);
        }
    }
}
