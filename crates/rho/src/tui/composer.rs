use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    commands, expand_paste_segments, input_cursor_index_on_visual_line, input_cursor_position,
    input_visual_lines, normalize_paste, paste_marker_for, previous_word_boundary, App,
    CommandInvocation, ComposerMode, HistoryDirection, InputDraft, InputSubmissionMode,
    PasteBurstEnter, PasteBurstKey, PasteSegment,
};

impl App {
    pub(super) fn flush_due_paste_burst(&mut self) -> bool {
        if self.paste_burst.is_due(Instant::now()) {
            self.flush_pending_paste_burst();
            true
        } else {
            false
        }
    }

    pub(super) fn flush_pending_paste_burst(&mut self) {
        let Some(text) = self.paste_burst.take_pending() else {
            return;
        };
        let text = normalize_paste(&text);
        self.insert_paste(&text);
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
                if !self.paste_burst.can_continue(now) {
                    self.flush_pending_paste_burst();
                }
                self.paste_burst.push_plain_char(ch, now);
                self.ctrl_c_streak = 0;
                true
            }
            PasteBurstKey::Enter => match self.paste_burst.push_enter_if_paste(now) {
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
            },
        }
    }

    fn insert_paste_burst_newline(&mut self) {
        match &mut self.composer {
            ComposerMode::Input => self.insert_input_char('\n'),
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.insert_char('\n');
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::OAuthPending(_) => {}
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
        match &self.composer {
            ComposerMode::Input => true,
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.accepts_paste_burst_char(ch)
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::OAuthPending(_) => false,
        }
    }

    fn composer_accepts_paste_burst_enter(&self) -> bool {
        match &self.composer {
            ComposerMode::Input => true,
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.active_text_entry_active()
                    || (self.paste_burst.has_pending()
                        && questionnaire.accepts_pending_paste_burst_enter())
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::OAuthPending(_) => false,
        }
    }

    pub(super) fn input_char_len(&self) -> usize {
        self.input.chars().count()
    }

    fn input_byte_index(&self, char_index: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.input.len())
    }

    pub(super) fn reset_input_history_navigation(&mut self) {
        self.input_history_cursor = None;
        self.input_history_draft = None;
    }

    pub(super) fn push_input_history(&mut self, prompt: &str) {
        self.reset_input_history_navigation();
        if prompt.is_empty() || self.input_history.last().is_some_and(|last| last == prompt) {
            return;
        }
        self.input_history.push(prompt.to_string());
    }

    fn recall_input_history(&mut self, direction: HistoryDirection) -> bool {
        if self.input_history.is_empty() {
            return false;
        }

        let next_cursor = match (direction, self.input_history_cursor) {
            (HistoryDirection::Previous, None) => {
                self.input_history_draft = Some(InputDraft {
                    input: self.input.clone(),
                    paste_segments: self.paste_segments.clone(),
                    submission_mode: self.input_submission_mode,
                    shell_mode: self.shell_mode,
                });
                self.input_history.len() - 1
            }
            (HistoryDirection::Previous, Some(0)) => 0,
            (HistoryDirection::Previous, Some(cursor)) => cursor - 1,
            (HistoryDirection::Next, None) => return false,
            (HistoryDirection::Next, Some(cursor)) if cursor + 1 < self.input_history.len() => {
                cursor + 1
            }
            (HistoryDirection::Next, Some(_)) => {
                let draft = self.input_history_draft.take().unwrap_or(InputDraft {
                    input: String::new(),
                    paste_segments: Vec::new(),
                    submission_mode: InputSubmissionMode::ParseCommands,
                    shell_mode: None,
                });
                self.shell_mode = draft.shell_mode;
                self.input = draft.input;
                self.paste_segments = draft.paste_segments;
                self.input_submission_mode = draft.submission_mode;
                self.input_cursor = self.input_char_len();
                self.input_history_cursor = None;
                self.input_changed();
                return true;
            }
        };

        self.apply_composer_text(
            self.input_history[next_cursor].clone(),
            Vec::new(),
            InputSubmissionMode::ParseCommands,
        );
        self.input_history_cursor = Some(next_cursor);
        true
    }

    pub(super) fn recall_input_history_or_move_cursor(
        &mut self,
        direction: HistoryDirection,
        terminal_width: usize,
    ) {
        let visual_lines = input_visual_lines(&self.input, terminal_width);
        let cursor_position = input_cursor_position(&self.input, self.input_cursor, terminal_width);
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
        self.input_cursor = input_cursor_index_on_visual_line(
            &self.input,
            &visual_lines,
            target_row,
            cursor_position.x as usize,
        );
        self.focus_paste_segment_at_cursor();
    }

    pub(super) fn move_input_cursor_left(&mut self) {
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start < self.input_cursor && self.input_cursor <= segment.end())
        {
            self.input_cursor = segment.start;
        } else {
            self.input_cursor = self.input_cursor.saturating_sub(1);
        }
    }

    pub(super) fn move_input_cursor_right(&mut self) {
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start <= self.input_cursor && self.input_cursor < segment.end())
        {
            self.input_cursor = segment.end();
        } else {
            self.input_cursor = (self.input_cursor + 1).min(self.input_char_len());
        }
    }

    fn focus_paste_segment_at_cursor(&mut self) {
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start < self.input_cursor && self.input_cursor < segment.end())
        {
            self.input_cursor = segment.start;
        }
    }

    pub(super) fn focused_paste_segment(&self) -> Option<&PasteSegment> {
        self.paste_segments
            .iter()
            .find(|segment| segment.start == self.input_cursor)
    }

    pub(super) fn replace_input_range(&mut self, start: usize, end: usize, text: &str) {
        self.reset_input_history_navigation();
        self.adjust_paste_segments_for_edit(start, end.saturating_sub(start), text.chars().count());
        let start_byte = self.input_byte_index(start);
        let end_byte = self.input_byte_index(end);
        self.input.replace_range(start_byte..end_byte, text);
        self.input_cursor = start + text.chars().count();
        self.input_changed();
    }

    pub(super) fn insert_input_char(&mut self, ch: char) {
        if ch == '!' && self.try_enter_shell_mode_from_bang() {
            return;
        }
        self.reset_input_history_navigation();
        self.adjust_paste_segments_for_edit(self.input_cursor, 0, 1);
        let byte_index = self.input_byte_index(self.input_cursor);
        self.input.insert(byte_index, ch);
        self.input_cursor += 1;
        self.input_changed();
    }

    pub(super) fn insert_input_text(&mut self, text: &str) {
        self.insert_input_text_with_paste_content(text, None);
    }

    pub(super) fn insert_pasted_input_text(&mut self, text: &str) {
        let Some(marker) = paste_marker_for(text) else {
            self.insert_input_text(text);
            return;
        };
        self.insert_input_text_with_paste_content(&marker, Some(text.to_string()));
    }

    fn insert_input_text_with_paste_content(&mut self, text: &str, paste_content: Option<String>) {
        self.reset_input_history_navigation();
        let start = self.input_cursor;
        let inserted_len = text.chars().count();
        self.adjust_paste_segments_for_edit(start, 0, inserted_len);
        let byte_index = self.input_byte_index(start);
        self.input.insert_str(byte_index, text);
        self.input_cursor += inserted_len;
        if let Some(content) = paste_content {
            self.paste_segments.push(PasteSegment {
                start,
                marker_len: inserted_len,
                content,
            });
            self.paste_segments.sort_by_key(|segment| segment.start);
        }
        self.input_changed();
    }

    pub(super) fn expanded_input(&self) -> String {
        expand_paste_segments(&self.input, &self.paste_segments)
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

    pub(super) fn backspace_input(&mut self) {
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start < self.input_cursor && self.input_cursor <= segment.end())
            .cloned()
        {
            self.replace_input_range(segment.start, segment.end(), "");
            return;
        }
        if self.input_cursor == 0 {
            if self.input.is_empty() && self.pending_images.pop().is_some() {
                self.status = format!("attached images: {}", self.pending_images.len());
            }
            return;
        }
        self.reset_input_history_navigation();
        let edit_start = self.input_cursor - 1;
        self.adjust_paste_segments_for_edit(edit_start, 1, 0);
        let start = self.input_byte_index(edit_start);
        let end = self.input_byte_index(self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor -= 1;
        self.input_changed();
    }

    pub(super) fn delete_input(&mut self) {
        if let Some(segment) = self
            .paste_segments
            .iter()
            .find(|segment| segment.start <= self.input_cursor && self.input_cursor < segment.end())
            .cloned()
        {
            self.replace_input_range(segment.start, segment.end(), "");
            return;
        }
        if self.input_cursor >= self.input_char_len() {
            return;
        }
        self.reset_input_history_navigation();
        self.adjust_paste_segments_for_edit(self.input_cursor, 1, 0);
        let start = self.input_byte_index(self.input_cursor);
        let end = self.input_byte_index(self.input_cursor + 1);
        self.input.replace_range(start..end, "");
        self.input_changed();
    }

    pub(super) fn delete_word_before_cursor(&mut self) {
        self.reset_input_history_navigation();
        let start_cursor = previous_word_boundary(&self.input, self.input_cursor);
        self.adjust_paste_segments_for_edit(start_cursor, self.input_cursor - start_cursor, 0);
        let start = self.input_byte_index(start_cursor);
        let end = self.input_byte_index(self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor = start_cursor;
        self.input_changed();
    }

    pub(super) fn input_changed(&mut self) {
        self.command_palette_dismissed = false;
        self.file_palette_dismissed = false;
        self.clamp_command_selection();
        self.clamp_file_selection();
    }

    pub(super) fn parse_input_command(
        &mut self,
    ) -> Result<Option<CommandInvocation>, commands::CommandParseError> {
        match std::mem::take(&mut self.input_submission_mode) {
            InputSubmissionMode::ParseCommands => {
                let result = commands::parse_command(&self.input);
                if matches!(result, Ok(Some(_))) {
                    let command = self.input.trim_end().to_string();
                    self.push_input_history(&command);
                }
                result
            }
            InputSubmissionMode::Prompt => Ok(None),
        }
    }

    pub(super) fn command_palette_visible(&self) -> bool {
        matches!(self.composer, ComposerMode::Input)
            && self.shell_mode.is_none()
            && !self.command_palette_dismissed
            && (self.cursor_in_command_token()
                || !commands::argument_choices(&self.input, self.input_cursor).is_empty())
            && !self.command_matches().is_empty()
    }

    fn cursor_in_command_token(&self) -> bool {
        if !self.input.starts_with('/') {
            return false;
        }

        let token_len = self
            .input
            .chars()
            .position(char::is_whitespace)
            .unwrap_or_else(|| self.input_char_len());
        self.input_cursor <= token_len
    }

    pub(super) fn clamp_command_selection(&mut self) {
        let prefix = self
            .cursor_in_command_token()
            .then(|| commands::command_prefix(&self.input).map(str::to_ascii_lowercase))
            .flatten();
        if self.command_prefix != prefix {
            self.command_prefix = prefix;
            self.command_selection = 0;
        }
        if self.command_prefix.is_some() {
            self.refresh_skill_match_cache();
        }

        let match_count = self.command_matches().len();
        if match_count == 0 {
            self.command_selection = 0;
        } else if self.command_selection >= match_count {
            self.command_selection = match_count - 1;
        }
    }
}
