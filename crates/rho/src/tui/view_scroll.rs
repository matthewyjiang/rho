//! History scrolling, scrollbar chrome, and jump-to-bottom for the transcript view.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    backend::Backend,
    text::{Line, Span},
    Terminal,
};

use super::{
    activity, App, ComposerMode, HistoryScrollbar, Theme, HISTORY_SCROLLBAR_REVEAL_DURATION,
};

impl App {
    pub(super) fn scroll_history_to_bottom(&mut self) {
        self.history.scroll_to_bottom();
    }

    pub(super) fn scroll_history_page_up(&mut self, width: usize, height: usize, now: Instant) {
        let page = self
            .history_content_height_for_screen(width, height, now)
            .max(1);
        self.scroll_history_lines(width, height, now, -(page as isize));
    }

    fn scroll_history_page_down(&mut self, width: usize, height: usize, now: Instant) {
        let page = self
            .history_content_height_for_screen(width, height, now)
            .max(1);
        self.scroll_history_lines(width, height, now, page as isize);
    }

    pub(super) fn scroll_history_lines(
        &mut self,
        width: usize,
        height: usize,
        now: Instant,
        delta: isize,
    ) {
        let history_len = self.history_len(width, now);
        let composer_line_count = self.composer_lines(width, height).len();
        let command_line_count = self.command_suggestion_lines(width).len();
        let content_height = self.history_content_height(self.history_height_from_line_counts(
            height,
            composer_line_count,
            command_line_count,
        ));
        self.history
            .scroll_chrome_mut()
            .scroll_by(history_len, content_height, delta);
    }

    pub(super) fn reveal_history_scrollbar(&mut self, now: Instant) {
        self.history
            .reveal_scrollbar(now, HISTORY_SCROLLBAR_REVEAL_DURATION);
    }

    pub(super) fn hide_history_scrollbar(&mut self) {
        self.history.hide_scrollbar();
    }

    pub(super) fn should_render_history_scrollbar(&self, now: Instant) -> bool {
        self.history.should_render_scrollbar(now)
    }

    pub(super) fn update_history_scrollbar_hover(
        &mut self,
        scrollbar: Option<HistoryScrollbar>,
        column: u16,
        row: u16,
    ) {
        self.history
            .scroll_chrome_mut()
            .update_hover(scrollbar, column, row);
    }

    pub(super) fn clamp_history_scroll(&mut self, width: usize, height: usize, now: Instant) {
        let history_len = self.history_len(width, now);
        let composer_line_count = self.composer_lines(width, height).len();
        let command_line_count = self.command_suggestion_lines(width).len();
        let content_height = self.history_content_height(self.history_height_from_line_counts(
            height,
            composer_line_count,
            command_line_count,
        ));
        self.history
            .scroll_chrome_mut()
            .clamp(history_len, content_height);
    }

    pub(super) fn clamp_history_scroll_for_terminal<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let size = terminal.size()?;
        self.note_terminal_geometry(size.width as usize, size.height as usize);
        self.clamp_history_scroll(size.width as usize, size.height as usize, Instant::now());
        Ok(())
    }

    pub(super) fn jump_to_bottom_line(&self, width: usize) -> Line<'static> {
        let text = self.jump_to_bottom_text(width);
        let binding = self.info.runtime.keybindings.jump_to_bottom.to_string();
        let Some(action) = text.strip_suffix(&binding) else {
            return Line::styled(text, Theme::jump_to_bottom());
        };
        Line::from(vec![
            Span::styled(action.to_string(), Theme::jump_to_bottom()),
            Span::styled(binding, Theme::jump_to_bottom_shortcut()),
        ])
    }

    pub(super) fn jump_to_bottom_text(&self, width: usize) -> String {
        activity::jump_to_bottom_text(
            width,
            &self.info.runtime.keybindings.jump_to_bottom.to_string(),
            /*alongside_activity*/ self.activity_status().is_some(),
        )
    }

    pub(super) fn handle_history_key<B: Backend>(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<B>,
    ) -> Result<bool, B::Error> {
        if matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_overlay()
        ) {
            return Ok(false);
        }
        let size = terminal.size()?;
        let width = size.width as usize;
        let height = size.height as usize;
        self.note_terminal_geometry(width, height);
        let now = Instant::now();
        match (key.modifiers, key.code) {
            (_, KeyCode::PageUp) => {
                self.reveal_history_scrollbar(now);
                self.history.set_scrollbar_drag(None);
                self.scroll_history_page_up(width, height, now);
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::PageDown) => {
                self.reveal_history_scrollbar(now);
                self.history.set_scrollbar_drag(None);
                self.scroll_history_page_down(width, height, now);
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ if self.info.runtime.keybindings.jump_to_bottom.matches(key) => {
                self.scroll_history_to_bottom();
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
