use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keybindings::ReservedComposerKey;

use super::{
    commands,
    composer_buffer::{ComposerEditKey, EditOutcome},
    composer_history::{history_step, HistoryStep},
    composer_layout::{content_width, prompt_width},
    paste_burst::{collapsed_paste_for, normalize_paste},
    App, CommandInvocation, ComposerMode, HistoryDirection, InputDraft, InputSubmissionMode,
    PasteBurstEnter, PasteBurstKey, PasteSegment,
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
            ComposerMode::Side => self.insert_side_paste_newline(),
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.insert_char('\n');
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::TextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::Panel(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::InteractivePending(_) => {}
        }
    }

    fn paste_burst_key(&self, key: KeyEvent) -> Option<PasteBurstKey> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Char(ch))
                if ReservedComposerKey::from_key(key)
                    == Some(ReservedComposerKey::TextInput(ch))
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
            ComposerMode::Input | ComposerMode::Side => true,
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.accepts_paste_burst_char(ch)
            }
            ComposerMode::Approval(_)
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::TextInput(_)
            | ComposerMode::Picker(_)
            | ComposerMode::Panel(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::InteractivePending(_) => false,
        }
    }

    fn composer_accepts_paste_burst_enter(&self) -> bool {
        match self.input_ui.composer() {
            ComposerMode::Input | ComposerMode::Side => true,
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
            | ComposerMode::Panel(_)
            | ComposerMode::InlineChoice(_)
            | ComposerMode::InteractivePending(_) => false,
        }
    }

    pub(super) fn input_char_len(&self) -> usize {
        self.input_ui.char_len()
    }

    pub(super) fn reset_input_history_navigation(&mut self) {
        self.input_ui.reset_history_navigation();
    }

    pub(super) fn push_input_history(&mut self, prompt: &str) {
        self.reset_input_history_navigation();
        if prompt.is_empty() {
            return;
        }
        if self.input_ui.push_history_if_new(prompt) && !self.info.session.no_save {
            self.prompt_history.push(prompt);
        }
    }

    fn recall_input_history(&mut self, direction: HistoryDirection) -> bool {
        let Some(step) = history_step(
            direction,
            self.input_ui.history().len(),
            self.input_ui.history_cursor(),
        ) else {
            return false;
        };
        match step {
            HistoryStep::Recall { index, .. } => self.recall_history_entry(index),
            HistoryStep::RestoreDraft => {
                let draft = self.input_ui.take_history_draft().unwrap_or(InputDraft {
                    input: String::new(),
                    paste_segments: Vec::new(),
                    submission_mode: InputSubmissionMode::ParseCommands,
                    shell_mode: None,
                });
                self.input_ui.apply_input_draft(draft);
                self.input_ui.set_history_cursor(None);
                // The draft was live typed text, so palettes reopen if it
                // still looks like a command or file mention.
                self.input_changed();
            }
        }
        true
    }

    /// Shows history entry `index` in the composer, as Up/Down would. Leaving
    /// the live draft saves it so stepping past the newest entry restores it.
    pub(super) fn recall_history_entry(&mut self, index: usize) {
        if self.input_ui.history_cursor().is_none() {
            self.input_ui.set_history_draft(Some(InputDraft {
                input: self.input_ui.text().to_string(),
                paste_segments: self.input_ui.paste_segments().to_vec(),
                submission_mode: self.input_ui.submission_mode(),
                shell_mode: self.input_ui.shell_mode(),
            }));
        }
        self.apply_composer_text(
            self.input_ui.history()[index].clone(),
            Vec::new(),
            InputSubmissionMode::ParseCommands,
        );
        self.input_ui.set_history_cursor(Some(index));
        // Recalled text is finished content, not a live search. Keep both
        // palettes closed until the next typed edit.
        self.input_ui.set_command_palette_dismissed(true);
        self.input_ui.set_file_palette_dismissed(true);
    }

    /// Apply a shared edit key to the main composer, with its history,
    /// attachment, shell-mode, and palette side effects.
    pub(super) fn apply_input_edit_key(&mut self, edit: ComposerEditKey) {
        match edit {
            ComposerEditKey::Backspace if self.input_ui.text().is_empty() => {
                if let Some(last) = self.input_ui.attachment_slots().len().checked_sub(1) {
                    self.remove_composer_attachment(last);
                }
                return;
            }
            ComposerEditKey::Char('!') if self.try_enter_shell_mode_from_bang() => return,
            ComposerEditKey::Home | ComposerEditKey::End => self.reset_input_history_navigation(),
            ComposerEditKey::Newline => self.input_ui.clear_paste_burst(),
            _ => {}
        }
        match self.input_ui.buffer_mut().apply_edit(edit) {
            EditOutcome::Edited => self.input_edited(),
            EditOutcome::VerticalEdge(direction) => {
                if !self.recall_input_history(direction) {
                    self.input_ui.buffer_mut().move_vertically(direction);
                }
            }
            EditOutcome::TextUnchanged => {}
        }
    }

    pub(super) fn focused_paste_segment(&self) -> Option<&PasteSegment> {
        self.input_ui.buffer().focused_paste_segment()
    }

    pub(super) fn replace_input_range(&mut self, start: usize, end: usize, text: &str) {
        self.input_ui
            .buffer_mut()
            .replace_range(start, end, text, /*paste_content*/ None);
        self.input_edited();
    }

    /// Bookkeeping after any main-composer text edit.
    fn input_edited(&mut self) {
        self.reset_input_history_navigation();
        self.input_changed();
    }

    pub(super) fn insert_input_char(&mut self, ch: char) {
        self.apply_input_edit_key(ComposerEditKey::Char(ch));
    }

    /// Insert plain composer text through the char path so rules like shell-mode
    /// bang handling stay single-sourced. Paste-burst flushes land here; collapsed
    /// paste markers use [`Self::insert_pasted_input_text`] instead.
    pub(super) fn insert_input_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.input_ui.buffer_mut().replace_selection(text) {
            self.input_edited();
            return;
        }
        for ch in text.chars() {
            self.insert_input_char(ch);
        }
    }

    pub(super) fn insert_pasted_input_text(&mut self, text: &str) {
        let Some(paste) = collapsed_paste_for(text) else {
            self.insert_input_text(text);
            return;
        };
        // A collapsed paste hides its content behind a marker; confirm the
        // catch so a large paste never looks like it silently vanished.
        self.notify_status(paste.toast());
        self.input_ui
            .buffer_mut()
            .insert_collapsed_paste(&paste, text);
        self.input_edited();
    }

    pub(super) fn expanded_input(&self) -> String {
        self.input_ui.expanded_text()
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

        let attachment_rows = self.composer_attachment_row_count(composer.width as usize);
        let visible_row = row.saturating_sub(composer.y) as usize;
        let absolute_row = layout.composer_start.saturating_add(visible_row);
        if absolute_row < attachment_rows {
            // Attachment labels / image previews are not part of the text buffer.
            return None;
        }
        let text_row = absolute_row.saturating_sub(attachment_rows);
        let width = composer.width as usize;
        let content_column =
            (column.saturating_sub(composer.x) as usize).saturating_sub(prompt_width());
        Some(
            self.input_ui
                .buffer()
                .char_index_at(content_width(width), text_row, content_column),
        )
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

    pub(super) fn cursor_in_command_token(&self) -> bool {
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
            self.input_ui.reset_command_selection();
        }

        let match_count = self.command_matches().len();
        self.input_ui.clamp_command_selection_to(match_count);
    }

    pub(super) fn insert_paste(&mut self, text: &str) {
        if self.insert_side_paste(text) {
            return;
        }
        match self.input_ui.composer_mut() {
            ComposerMode::Input => self.insert_pasted_input_text(text),
            ComposerMode::SecretInput(secret) => secret.editor.insert_text(text),
            ComposerMode::ConfigNumberInput(input) => input.insert_text(text),
            ComposerMode::TextInput(input) => input.editor.insert_text(text),
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire.insert_text(text);
            }
            ComposerMode::Side => {}
            ComposerMode::Approval(_)
            | ComposerMode::Picker(_)
            | ComposerMode::Panel(_)
            | ComposerMode::InteractivePending(_)
            | ComposerMode::InlineChoice(_) => {}
        }
    }

    pub(super) fn apply_external_paste(&mut self, text: &str) {
        self.flush_pending_paste_burst();
        let text = normalize_paste(text);
        self.insert_external_paste(&text);
        self.input_ui.clear_paste_burst();
    }

    pub(super) fn insert_external_paste(&mut self, text: &str) {
        let is_command = matches!(commands::parse_command(text), Ok(Some(_)));
        if is_command || !self.start_pasted_media_path(text) {
            self.insert_paste(text);
        }
    }
}
