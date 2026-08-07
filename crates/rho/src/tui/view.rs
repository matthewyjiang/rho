use std::{sync::Arc, time::Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    backend::Backend,
    layout::{Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    DefaultTerminal, Frame, Terminal,
};

use super::{
    activity, history_cache::HistoryLineSlice, App, CachedCodeBlock, CodeBlockCopyTarget,
    ComposerMode, Entry, GoalStatus, HistoryScrollbar, LineFill, ReasoningChrome,
    SessionHeaderCache, StreamKind, Theme, HISTORY_SCROLLBAR_REVEAL_DURATION,
    RECOVERED_HISTORY_LINE_LIMIT,
};
use super::{
    highlight_selection,
    message_history::{recovered_history_tail, transcript_entries_from_messages},
    picker_overlay::picker_overlay_frame,
    render::{pad_display_line, padded_content_width, truncate_one_line},
    render_copy_notice,
    screen_layout::{terminal_meets_minimum, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH},
    session_header_lines, styled_line, tool_entry_lines,
};
#[cfg(test)]
use super::{ActiveFrame, DEFAULT_TUI_HEIGHT};

#[derive(Clone, Copy)]
struct DrawSurface<'a> {
    area: Rect,
    width: usize,
    now: Instant,
    layout: &'a super::screen_layout::ScreenLayout,
}

fn draw_terminal_too_small(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = area.width as usize;
    let message = format!("terminal too small (need {MIN_TERMINAL_WIDTH}x{MIN_TERMINAL_HEIGHT})");
    let line = styled_line(
        truncate_one_line(&message, width),
        width,
        Theme::warning(),
        LineFill::Natural,
    );
    let y = area.y.saturating_add(area.height.saturating_sub(1) / 2);
    frame.render_widget(
        Paragraph::new(line).style(Style::default()),
        Rect::new(area.x, y, area.width, 1),
    );
}

impl App {
    pub(super) fn draw(&mut self, frame: &mut Frame<'_>) {
        let now = Instant::now();
        let area = frame.area();
        if !terminal_meets_minimum(area) {
            draw_terminal_too_small(frame, area);
            return;
        }
        // First-launch setup owns the whole screen: no history, composer,
        // statusline, or hints until the user has a provider and a model.
        if let Some(step) = self.setup_step() {
            self.draw_setup_screen(frame, area, step);
            return;
        }
        let width = area.width as usize;
        let live_history = self.history_live_lines(width, now);
        let history_len = self.history_len_with_live(width, &live_history);
        let composer_lines = self.composer_lines(width, area.height as usize);
        let command_lines = self.command_suggestion_lines(width);
        let layout = self.screen_layout_for_history_len(
            area,
            history_len,
            &composer_lines,
            command_lines.len(),
        );
        let (history_start, history_count) =
            self.visible_history_window(history_len, layout.history_content.height as usize);
        self.draw_history(
            frame,
            width,
            &layout,
            history_start,
            history_count,
            &live_history,
        );
        let surface = DrawSurface {
            area,
            width,
            now,
            layout: &layout,
        };
        self.draw_panels(frame, surface);
        self.draw_composer(frame, surface, composer_lines, command_lines);
        self.draw_cursor(frame, surface);
        if let Some(selection) = self.screen_selection {
            highlight_selection(frame.buffer_mut(), area, 0, selection);
        }
    }

    fn draw_history(
        &mut self,
        frame: &mut Frame<'_>,
        width: usize,
        layout: &super::screen_layout::ScreenLayout,
        history_start: usize,
        history_count: usize,
        live_history: &[Line<'static>],
    ) {
        let history_visible =
            self.visible_history_lines_with_live(width, history_start, history_count, live_history);
        let visible_images =
            self.visible_history_image_placements(width, history_start, history_count);
        frame.render_widget(
            Paragraph::new(history_visible).style(Style::default()),
            layout.history_content,
        );
        if let Some(selection) = self.history.text_selection() {
            highlight_selection(
                frame.buffer_mut(),
                layout.history_content,
                history_start,
                selection,
            );
        }
        if let Some(hovered_line) = self.history.hovered_code_block_copy() {
            let code_block_copy_targets = self.code_block_copy_targets(width);
            if let Some(target) = code_block_copy_targets
                .iter()
                .find(|target| target.line == hovered_line)
                .filter(|target| {
                    (history_start..history_start + history_count).contains(&target.line)
                })
            {
                let row = layout
                    .history_content
                    .y
                    .saturating_add(target.line.saturating_sub(history_start) as u16);
                for column in target
                    .columns
                    .clone()
                    .take(layout.history_content.width as usize)
                {
                    frame.buffer_mut()
                        [(layout.history_content.x.saturating_add(column as u16), row)]
                        .set_style(Theme::markdown_code_copy_button(/*hovered*/ true));
                }
            }
        }
        self.render_feed_images(frame, layout.history_content, &visible_images);
    }

    fn draw_panels(&mut self, frame: &mut Frame<'_>, surface: DrawSurface<'_>) {
        let DrawSurface {
            width, now, layout, ..
        } = surface;
        if let Some(activity_gap) = layout.activity_gap {
            frame.render_widget(Clear, activity_gap);
        }
        if let Some(activity_rail) = layout.activity_rail {
            frame.render_widget(Clear, activity_rail);
            frame.render_widget(
                Paragraph::new("").style(Theme::activity_rail()),
                activity_rail,
            );
        }
        if let Some(scrollbar) = layout
            .history_scrollbar
            .filter(|_| self.should_render_history_scrollbar(now))
        {
            scrollbar.render(frame, self.history.scrollbar_drag().is_some());
        }
        if let Some(activity) = layout.activity {
            frame.render_widget(
                Paragraph::new(
                    self.turn.loading_spinner().line(
                        now,
                        activity.width as usize,
                        self.activity_status()
                            .expect("activity layout requires active status"),
                    ),
                )
                .style(Style::default()),
                activity,
            );
        }
        if let Some(button) = layout.jump_to_bottom {
            frame.render_widget(
                Paragraph::new(self.jump_to_bottom_line(width)).style(Style::default()),
                button,
            );
        }
        if layout.pending_input.height > 0 {
            frame.render_widget(
                Paragraph::new(
                    self.pending_input_lines(width)
                        .into_iter()
                        .take(layout.pending_input.height as usize)
                        .collect::<Vec<_>>(),
                )
                .style(Style::default()),
                layout.pending_input,
            );
        }
        if layout.subagents.height > 0 {
            frame.render_widget(
                Paragraph::new(self.subagent_panel.lines(
                    width,
                    layout.subagents.height as usize,
                    self.subagent_action_hint(),
                ))
                .style(Theme::activity_rail()),
                layout.subagents,
            );
            if let Some((row, state)) = self.subagent_panel.highlighted_row() {
                let y = layout.subagents.y.saturating_add(row as u16);
                if y < layout.subagents.bottom() {
                    for x in layout.subagents.x..layout.subagents.right() {
                        frame.buffer_mut()[(x, y)].set_style(Theme::subagent_row(state));
                    }
                }
            }
        }
        if layout.top_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width, /*shell_label*/ true)])
                    .style(Style::default()),
                layout.top_divider,
            );
        }
    }

    fn draw_composer(
        &mut self,
        frame: &mut Frame<'_>,
        surface: DrawSurface<'_>,
        composer_lines: Vec<Line<'static>>,
        command_lines: Vec<Line<'static>>,
    ) {
        let DrawSurface {
            area,
            width,
            now,
            layout,
        } = surface;
        let composer_visible = composer_lines
            .into_iter()
            .skip(layout.composer_start)
            .take(layout.composer.height as usize)
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(
                command_lines
                    .into_iter()
                    .take(layout.commands.height as usize)
                    .collect::<Vec<_>>(),
            )
            .style(Style::default()),
            layout.commands,
        );
        frame.render_widget(
            Paragraph::new(composer_visible).style(Style::default()),
            layout.composer,
        );
        if layout.bottom_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width, /*shell_label*/ false)])
                    .style(Style::default()),
                layout.bottom_divider,
            );
        }
        let statusline_height = layout.statusline.height as usize;
        for (index, line) in self
            .statusline_lines(width)
            .iter()
            .take(statusline_height)
            .enumerate()
        {
            let row = Rect::new(
                layout.statusline.x,
                layout.statusline.y.saturating_add(index as u16),
                layout.statusline.width,
                1,
            );
            frame.render_widget(line, row);
        }
        if let Some(notice) = &self.history.copy_notice() {
            render_copy_notice(frame, area, notice, now);
        }
        if let Some(overlay) = self.status_overlay.as_ref() {
            let copy_offset = self
                .history
                .copy_notice()
                .filter(|notice| notice.is_visible(now))
                .map(|_| area.height.min(3))
                .unwrap_or(0);
            super::status_overlay::render_status_overlay(
                frame,
                area,
                overlay,
                now,
                /*top_offset*/ copy_offset,
            );
        }
    }

    fn draw_cursor(&self, frame: &mut Frame<'_>, surface: DrawSurface<'_>) {
        let DrawSurface {
            area,
            width,
            layout,
            ..
        } = surface;
        let popup_cursor = if let ComposerMode::Picker(picker) = self.input_ui.composer() {
            picker_overlay_frame(picker, area).map(|overlay| {
                frame.render_widget(Clear, overlay.outer);
                frame.render_widget(
                    Paragraph::new(overlay.lines).style(Style::default()),
                    overlay.outer,
                );
                overlay.cursor
            })
        } else {
            None
        };

        if let Some(position) = popup_cursor {
            frame.set_cursor_position(position);
            return;
        }

        // A zero-high composer owns no row; do not park the cursor on foreign chrome.
        if layout.composer.height == 0 {
            return;
        }

        let full_cursor = self.composer_cursor_position(width);
        let max_cursor_x = width.max(1).saturating_sub(1) as u16;
        let cursor_y = full_cursor
            .y
            .saturating_sub(layout.composer_start as u16)
            .min(layout.composer.height.saturating_sub(1));
        frame.set_cursor_position(Position {
            x: layout
                .composer
                .x
                .saturating_add(full_cursor.x.min(max_cursor_x)),
            y: layout.composer.y.saturating_add(cursor_y),
        });
    }

    #[cfg(test)]
    pub(super) fn active_lines(&mut self, width: usize) -> Vec<Line<'static>> {
        self.active_lines_at_for_height(width, DEFAULT_TUI_HEIGHT as usize, Instant::now())
    }

    #[cfg(test)]
    pub(super) fn active_lines_at_for_height(
        &mut self,
        width: usize,
        viewport_height: usize,
        now: Instant,
    ) -> Vec<Line<'static>> {
        self.active_frame_at_for_height(width, viewport_height, now)
            .lines
    }

    #[cfg(test)]
    fn active_frame_at_for_height(
        &mut self,
        width: usize,
        viewport_height: usize,
        now: Instant,
    ) -> ActiveFrame {
        let area = Rect::new(0, 0, width as u16, viewport_height as u16);
        let history_len = self.history_len(width, now);
        let composer_lines = self.composer_lines(width, area.height as usize);
        let command_lines = self.command_suggestion_lines(width);
        let layout = self.screen_layout_for_history_len(
            area,
            history_len,
            &composer_lines,
            command_lines.len(),
        );
        let (history_start, history_count) =
            self.visible_history_window(history_len, layout.history_content.height as usize);
        let mut lines = self.visible_history_lines(width, now, history_start, history_count);
        lines.resize(layout.history.height as usize, Line::default());
        if let Some(activity) = layout.activity {
            lines[activity.y.saturating_sub(layout.history.y) as usize] =
                self.turn.loading_spinner().line(
                    now,
                    activity.width as usize,
                    self.activity_status()
                        .expect("activity layout requires active status"),
                );
        }
        if let Some(button) = layout.jump_to_bottom {
            lines[button.y.saturating_sub(layout.history.y) as usize] =
                self.jump_to_bottom_line(width);
        }
        if layout.pending_input.height > 0 {
            lines.extend(
                self.pending_input_lines(width)
                    .into_iter()
                    .take(layout.pending_input.height as usize),
            );
        }
        if layout.subagents.height > 0 {
            lines.extend(self.subagent_panel.lines(
                width,
                layout.subagents.height as usize,
                self.subagent_action_hint(),
            ));
        }
        if layout.top_divider.height > 0 {
            lines.push(self.divider_line(width, /*shell_label*/ true));
        }
        lines.extend(
            command_lines
                .into_iter()
                .take(layout.commands.height as usize),
        );
        lines.extend(
            composer_lines
                .into_iter()
                .skip(layout.composer_start)
                .take(layout.composer.height as usize),
        );
        if layout.bottom_divider.height > 0 {
            lines.push(self.divider_line(width, /*shell_label*/ false));
        }
        lines.extend(
            self.statusline_lines(width)
                .iter()
                .take(layout.statusline.height as usize)
                .cloned(),
        );

        ActiveFrame { lines }
    }

    pub(super) fn session_header_lines(&mut self, width: usize) -> &[Line<'static>] {
        let update_notice = self.info.services.update_notice.clone();
        let setup = self.setup_state();
        let stale = self.history.session_header_cache().is_none_or(|cache| {
            cache.width != width || cache.update_notice != update_notice || cache.setup != setup
        });
        if stale {
            self.history
                .set_session_header_cache(Some(SessionHeaderCache {
                    width,
                    update_notice,
                    setup,
                    lines: session_header_lines(
                        self.info.services.update_notice.as_deref(),
                        setup,
                        width,
                    ),
                }));
        }
        &self.history.session_header_cache().unwrap().lines
    }

    pub(super) fn history_len(&mut self, width: usize, now: Instant) -> usize {
        let live = self.history_live_lines(width, now);
        self.history_len_with_live(width, &live)
    }

    fn history_len_with_live(&mut self, width: usize, live: &[Line<'static>]) -> usize {
        self.history_static_len(width).saturating_add(live.len())
    }

    pub(super) fn visible_history_lines(
        &mut self,
        width: usize,
        now: Instant,
        start: usize,
        count: usize,
    ) -> Vec<Line<'static>> {
        let live = self.history_live_lines(width, now);
        self.visible_history_lines_with_live(width, start, count, &live)
    }

    fn visible_history_lines_with_live(
        &mut self,
        width: usize,
        start: usize,
        count: usize,
        live: &[Line<'static>],
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if count == 0 {
            return lines;
        }

        let header_lines = self.session_header_lines(width).to_vec();
        let header_len = header_lines.len();
        if start < header_len {
            let header_count = count.min(header_len - start);
            lines.extend(header_lines[start..start + header_count].iter().cloned());
        }

        if lines.len() < count {
            let transcript_start = start.saturating_sub(header_len);
            let transcript_count = count - lines.len();
            let cwd = self.info.runtime.cwd.clone();
            self.sync_open_stream_tail();
            self.history
                .with_lines_and_images_mut(|history_lines, entries, markdown_images| {
                    history_lines.extend_visible_lines(
                        entries,
                        self.info.runtime.history_render_settings(width),
                        HistoryLineSlice {
                            start: transcript_start,
                            count: transcript_count,
                        },
                        &mut lines,
                        &|entry_index, sources| {
                            markdown_images.ready_images(entry_index, sources, &cwd)
                        },
                    );
                });
        }

        let static_len = header_len.saturating_add(self.cached_transcript_line_count(width));
        if lines.len() < count {
            let live_start = start.saturating_sub(static_len);
            lines.extend(
                live.iter()
                    .skip(live_start)
                    .take(count - lines.len())
                    .cloned(),
            );
        }
        lines
    }

    /// Open assistant/reasoning entries omit their trailing separator while the stream is live.
    pub(super) fn sync_open_stream_tail(&mut self) {
        let open = match self.streams.current_stream_kind {
            None => false,
            Some(StreamKind::Assistant) => matches!(self.history.last(), Some(Entry::Assistant(_))),
            Some(StreamKind::Reasoning) => {
                self.info.runtime.shows_work_chrome()
                    && matches!(
                        self.history.last(),
                        Some(Entry::Reasoning(reasoning)) if !reasoning.text.is_empty()
                    )
            }
        };
        self.history.lines_mut().set_open_stream_tail(open);
    }

    pub(super) fn history_static_len(&mut self, width: usize) -> usize {
        self.session_header_lines(width)
            .len()
            .saturating_add(self.cached_transcript_line_count(width))
    }

    pub(super) fn cached_transcript_line_count(&mut self, width: usize) -> usize {
        self.sync_open_stream_tail();
        let cwd = self.info.runtime.cwd.clone();
        let settings = self.info.runtime.history_render_settings(width);
        self.history
            .with_lines_and_images_mut(|history_lines, entries, markdown_images| {
                history_lines.line_count(entries, settings, &|entry_index, sources| {
                    markdown_images.ready_images(entry_index, sources, &cwd)
                })
            })
    }

    pub(super) fn code_block_copy_targets(&mut self, width: usize) -> Vec<CodeBlockCopyTarget> {
        self.sync_open_stream_tail();
        let header_len = self.session_header_lines(width).len();
        let cwd = self.info.runtime.cwd.clone();
        let settings = self.info.runtime.history_render_settings(width);
        self.history
            .with_lines_and_images_mut(|history_lines, entries, markdown_images| {
                history_lines
                    .code_blocks(entries, settings, &|entry_index, sources| {
                        markdown_images.ready_images(entry_index, sources, &cwd)
                    })
                    .iter()
                    .map(|block: &CachedCodeBlock| CodeBlockCopyTarget {
                        line: header_len.saturating_add(block.line),
                        columns: block.copy_columns.clone(),
                        text: Arc::clone(&block.text),
                    })
                    .collect()
            })
    }

    pub(super) fn history_live_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let show_tools = self.info.runtime.shows_work_chrome();
        let shells = if show_tools {
            self.running_inline_shell_entries().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let tools = if show_tools {
            self.turn.tool_calls().live_entries().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let has_pending_tools = !shells.is_empty() || !tools.is_empty();
        // Open stream tails omit the history trailing blank so previews can abut
        // committed text. Live tools still need one row of separation above them.
        if has_pending_tools && self.open_stream_tail_active() {
            lines.push(Line::raw(""));
        }
        for pending in &shells {
            // tool_entry_lines owns the trailing spacer under each card.
            lines.extend(tool_entry_lines(
                pending,
                width,
                self.info.runtime.max_tool_output_lines,
            ));
        }
        for pending in tools {
            lines.extend(tool_entry_lines(
                pending,
                width,
                self.info.runtime.max_tool_output_lines,
            ));
        }
        if let Some(preview) = &self.streams.live_stream_preview {
            let show_preview = match preview.kind {
                StreamKind::Assistant => true,
                StreamKind::Reasoning => self.info.runtime.displays_reasoning_output(),
            };
            if show_preview {
                lines.extend(self.render_stream_preview_lines(preview, width));
            }
        }
        if self.turn.reasoning_phase().is_open()
            && matches!(
                self.info.runtime.reasoning_chrome(),
                ReasoningChrome::ThinkingPlaceholder
            )
        {
            lines.push(Line::raw(""));
            lines.push(pad_display_line(styled_line(
                "Thinking...".into(),
                padded_content_width(width),
                StreamKind::Reasoning.style(),
                LineFill::Natural,
            )));
        }
        lines
    }

    pub(super) fn open_stream_tail_active(&self) -> bool {
        match self.streams.current_stream_kind {
            None => false,
            Some(StreamKind::Assistant) => matches!(self.history.last(), Some(Entry::Assistant(_))),
            Some(StreamKind::Reasoning) => matches!(
                self.history.last(),
                Some(Entry::Reasoning(reasoning)) if !reasoning.text.is_empty()
            ),
        }
    }

    pub(super) fn visible_history_window(
        &self,
        history_len: usize,
        content_height: usize,
    ) -> (usize, usize) {
        (
            self.visible_history_start(history_len, content_height),
            content_height,
        )
    }

    pub(super) fn visible_history_start(&self, history_len: usize, height: usize) -> usize {
        self.history
            .scroll_chrome()
            .visible_start(history_len, height)
    }

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

    fn goal_status(&self) -> Option<GoalStatus> {
        self.goal.as_ref().map(|goal| GoalStatus {
            turns: goal.turns,
            elapsed: goal.elapsed(),
            blocked: goal.is_blocked(),
        })
    }

    fn refresh_statusline_state(&mut self) {
        self.statusline
            .update_signed_in(self.setup_state().signed_in);
        self.statusline.update_model(&self.info.runtime);
        let display_usage = super::usage_cost::display_usage_with_live(
            self.usage.cumulative_usage.as_ref(),
            &self.usage.live_stream,
            self.model_metadata.as_ref(),
        );
        self.statusline.update_usage(
            display_usage.as_ref(),
            self.usage.current_context.as_ref(),
            self.usage.extra_cost_usd_micros(),
        );
        let performance = self
            .usage
            .model_performance
            .summary(&self.info.runtime.model_call_profile());
        self.statusline.update_average_generation_rate(
            performance
                .average_generation_tokens_per_second
                .map(|rate| rate.round() as u64),
        );
        self.statusline
            .update_model_metadata(self.model_metadata.as_ref());
    }

    pub(super) fn statusline_lines(&mut self, width: usize) -> &[Line<'static>] {
        let goal = self.goal_status();
        self.refresh_statusline_state();
        self.statusline.lines(width, goal)
    }

    pub(super) fn insert_recovered_history(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        let entries = transcript_entries_from_messages(
            &self.info.session.recovered_messages,
            &self.info.runtime.cwd,
        );
        if entries.is_empty() {
            return Ok(());
        }

        let width = terminal.size()?.width as usize;
        let (omitted, visible_entries) = recovered_history_tail(
            &entries,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.runtime.history_render_settings(width),
        );
        let mut transcript = Vec::new();
        if omitted > 0 {
            transcript.push(Entry::Notice(format!(
                "resumed session; showing last {} messages, omitted {omitted} earlier messages",
                visible_entries.len()
            )));
        }
        transcript.extend(visible_entries);
        self.history.set_entries(transcript);
        self.history.images_mut().clear();
        self.history.invalidate_from(0);
        Ok(())
    }
}
