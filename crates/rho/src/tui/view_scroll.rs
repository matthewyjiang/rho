//! History scrolling, scrollbar chrome, and jump-to-bottom for the transcript view.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    backend::Backend,
    text::{Line, Span},
    Terminal,
};

use super::{
    activity, history_cache::HistoryRenderSettings, scrollbar::unmeasured_prefix_scroll_need, App,
    HistoryScrollbar, Theme, HISTORY_SCROLLBAR_REVEAL_DURATION,
};

impl App {
    pub(super) fn ensure_measured_history_suffix(
        &mut self,
        settings: HistoryRenderSettings,
        viewport: usize,
    ) {
        // One extra pane so wheel/page-up does not wrap on the first notch.
        let min_lines = viewport.saturating_add(viewport.max(1));
        let cwd = self.info.runtime.cwd.clone();
        self.history
            .with_lines_and_images_mut(|cache, entries, images| {
                cache.ensure_suffix(entries, settings, min_lines, &|index, sources| {
                    images.ready_images(index, sources, &cwd)
                });
            });
    }

    pub(super) fn grow_measured_history_prefix(
        &mut self,
        settings: HistoryRenderSettings,
        extra_lines: usize,
    ) -> usize {
        let cwd = self.info.runtime.cwd.clone();
        self.history
            .with_lines_and_images_mut(|cache, entries, images| {
                cache.grow_prefix(entries, settings, extra_lines, &|index, sources| {
                    images.ready_images(index, sources, &cwd)
                })
            })
    }

    pub(super) fn scroll_history_to_bottom(&mut self) {
        self.history.scroll_to_bottom();
    }

    pub(super) fn scroll_history_lines(
        &mut self,
        width: usize,
        height: usize,
        now: Instant,
        delta: isize,
    ) {
        let content_height = self.history_content_height_for_screen(width, height, now);
        let settings = self.history_render_settings(width);
        self.ensure_measured_history_suffix(settings, content_height);
        let history_len = self.history_len(width, now);
        let start = self.visible_history_start(history_len, content_height);
        let had_unmeasured = self.history.has_unmeasured_prefix();
        let overflow = unmeasured_prefix_scroll_need(start, delta, had_unmeasured);
        if overflow > 0 {
            // One extra pane so the next wheel ticks do not wrap immediately.
            let prepended =
                self.grow_measured_history_prefix(settings, overflow.max(content_height.max(1)));
            let header_inserted = had_unmeasured && !self.history.has_unmeasured_prefix();
            let header_shift = if header_inserted {
                self.visible_session_header_len(width)
            } else {
                0
            };
            let new_len = self.history_len(width, now);
            let new_start = start
                .saturating_add(prepended)
                .saturating_add(header_shift)
                .saturating_add_signed(delta);
            self.history
                .scroll_chrome_mut()
                .set_top_line(new_len, content_height, new_start);
            return;
        }
        self.history
            .scroll_chrome_mut()
            .scroll_by(history_len, content_height, delta);
    }

    /// Dragging the bar to the measured top must reach the transcript start,
    /// the same as repeated page-up.
    pub(super) fn reveal_unmeasured_history_at_scrollbar_top(
        &mut self,
        width: usize,
        height: usize,
        now: Instant,
    ) {
        if !self.history.has_unmeasured_prefix() {
            return;
        }
        let content_height = self.history_content_height_for_screen(width, height, now);
        let history_len = self.history_len(width, now);
        if self.visible_history_start(history_len, content_height) > 0 {
            return;
        }
        let settings = self.history_render_settings(width);
        if self.grow_measured_history_prefix(settings, usize::MAX) == 0 {
            return;
        }
        let new_len = self.history_len(width, now);
        self.history
            .scroll_chrome_mut()
            .pin_top_line(new_len, content_height, 0);
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
        let content_height = self.history_content_height_for_screen(width, height, now);
        let settings = self.history_render_settings(width);
        self.ensure_measured_history_suffix(settings, content_height);
        let history_len = self.history_len(width, now);
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
        if self.input_ui.composer().is_centered_overlay() {
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
                let page = self
                    .history_content_height_for_screen(width, height, now)
                    .max(1);
                self.scroll_history_lines(width, height, now, -(page as isize));
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::PageDown) => {
                self.reveal_history_scrollbar(now);
                self.history.set_scrollbar_drag(None);
                let page = self
                    .history_content_height_for_screen(width, height, now)
                    .max(1);
                self.scroll_history_lines(width, height, now, page as isize);
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
