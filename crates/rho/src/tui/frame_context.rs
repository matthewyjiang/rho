//! One geometry path for draw, mouse, and scroll.
//!
//! Composer wrap, suggestion lines, history settings, and the screen layout are
//! computed together so every consumer of a frame sees the same chrome.

use std::time::Instant;

use ratatui::{layout::Rect, text::Line};

use super::{
    history_cache::HistoryRenderSettings,
    screen_layout::{interactive_chrome, ChromeRails, ScreenLayout},
    view::LiveHistory,
    view_composer::ComposerFrame,
    App,
};

/// Chrome, history settings, and layout for one terminal size.
pub(super) struct FrameContext {
    pub(super) width: usize,
    pub(super) composer: ComposerFrame,
    pub(super) command_lines: Vec<Line<'static>>,
    pub(super) settings: HistoryRenderSettings,
    pub(super) live_history: LiveHistory,
    pub(super) history_len: usize,
    pub(super) layout: ScreenLayout,
}

impl App {
    /// Compute the frame geometry used by draw, mouse, and scroll.
    ///
    /// Recomputed per event or frame. Do not cache across events: composer text,
    /// live history, and theme can all change between them.
    pub(super) fn frame_context(&mut self, area: Rect, now: Instant) -> FrameContext {
        let width = area.width as usize;
        let height = area.height as usize;
        let composer = self.composer_frame(width, height);
        let command_lines = self.command_suggestion_lines(width);
        let chrome = interactive_chrome(ChromeRails {
            height,
            desired_statusline_height: self.statusline.height(),
            composer_line_count: composer.lines.len(),
            command_line_count: command_lines.len(),
            desired_pending: self.pending_input_height(),
            desired_subagents: self.subagent_panel.desired_height(),
            desired_processes: self.process_panel.desired_height(),
            activity_floor: usize::from(
                self.subagent_panel.is_active() || self.process_panel.is_active(),
            ),
        });
        let content_height = self.history_content_height(chrome.history_height());
        let budget = super::feed_image::ImageRowBudget::feed(height, content_height).get();
        let settings = self.info.runtime.history_render_settings(width, budget);
        let live_history = self.live_history_layout(width, settings.max_image_height);
        self.ensure_measured_history_suffix(settings, content_height);
        let history_len = self
            .history_static_len(width, settings)
            .saturating_add(live_history.lines.len());
        let layout = self.build_screen_layout(
            area,
            history_len,
            &composer.lines,
            composer.cursor,
            chrome,
            now,
        );
        FrameContext {
            width,
            composer,
            command_lines,
            settings,
            live_history,
            history_len,
            layout,
        }
    }

    pub(super) fn history_static_len(
        &mut self,
        width: usize,
        settings: HistoryRenderSettings,
    ) -> usize {
        self.visible_session_header_len(width)
            .saturating_add(self.cached_transcript_line_count(settings))
    }
}
