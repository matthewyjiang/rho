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
    activity, file_picker, history_cache::HistoryLineSlice, inline_shell, App, CachedCodeBlock,
    CodeBlockCopyTarget, ComposerMode, Entry, GoalStatus, HistoryScroll, HistoryScrollbar,
    InlineShellMode, LineFill, SessionHeaderCache, StreamKind, Theme,
    HISTORY_SCROLLBAR_REVEAL_DURATION, MAX_COMMAND_SUGGESTIONS, MIN_COMMAND_DESCRIPTION_WIDTH,
    RECOVERED_HISTORY_LINE_LIMIT,
};
use super::{
    approval_lines, char_prefix_display_width, config_number_input_lines, config_text_input_lines,
    display_width, highlight_selection, input_cursor_position, input_lines_with_images,
    is_tool_entry, oauth_pending_lines, pad_display_line, padded_content_width, picker_lines,
    questionnaire_cursor_position, questionnaire_lines, recovered_history_tail, render_copy_notice,
    scroll_state_for_top_line, secret_input_lines, session_header_lines, styled_line,
    tool_entry_lines, transcript_entries_from_messages, truncate_one_line,
};
#[cfg(test)]
use super::{ActiveFrame, DEFAULT_TUI_HEIGHT};

impl App {
    pub(super) fn draw(&mut self, frame: &mut Frame<'_>) {
        let now = Instant::now();
        let area = frame.area();
        let width = area.width as usize;
        let live_history = self.history_live_lines(width, now);
        let history_len = self
            .history_static_len(width)
            .saturating_add(live_history.len());
        let composer_lines = self.composer_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        let layout = self.screen_layout_for_history_len(
            area,
            history_len,
            &composer_lines,
            command_lines.len(),
        );
        let (history_start, history_count) =
            self.visible_history_window(history_len, layout.history.height as usize);
        let history_visible = self.visible_history_lines_with_live(
            width,
            history_start,
            history_count,
            &live_history,
        );
        let visible_images =
            self.visible_history_image_placements(width, history_start, history_count);
        frame.render_widget(
            Paragraph::new(history_visible).style(Style::default()),
            layout.history,
        );
        if let Some(selection) = self.text_selection {
            highlight_selection(frame.buffer_mut(), layout.history, history_start, selection);
        }
        if let Some(hovered_line) = self.hovered_code_block_copy {
            let code_block_copy_targets = self.code_block_copy_targets(width);
            if let Some(target) = code_block_copy_targets
                .iter()
                .find(|target| target.line == hovered_line)
                .filter(|target| {
                    (history_start..history_start + layout.history.height as usize)
                        .contains(&target.line)
                })
            {
                let row = layout
                    .history
                    .y
                    .saturating_add(target.line.saturating_sub(history_start) as u16);
                for column in target.columns.clone().take(layout.history.width as usize) {
                    frame.buffer_mut()[(layout.history.x.saturating_add(column as u16), row)]
                        .set_style(Theme::markdown_code_copy_button(/*hovered*/ true));
                }
            }
        }
        self.render_feed_images(frame, layout.history, &visible_images);
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
            scrollbar.render(frame, self.history_scrollbar_drag.is_some());
        }
        if let Some(activity) = layout.activity {
            frame.render_widget(
                Paragraph::new(
                    self.loading_spinner.line(
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
                Paragraph::new(
                    self.subagent_panel
                        .lines(width, layout.subagents.height as usize),
                )
                .style(Theme::activity_rail()),
                layout.subagents,
            );
        }
        if layout.top_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width)]).style(Style::default()),
                layout.top_divider,
            );
        }

        let composer_visible = composer_lines
            .into_iter()
            .skip(layout.composer_start)
            .take(layout.composer.height as usize)
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(composer_visible).style(Style::default()),
            layout.composer,
        );
        if layout.bottom_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width)]).style(Style::default()),
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
        if let Some(notice) = &self.copy_notice {
            render_copy_notice(frame, area, notice, now);
        }

        let full_cursor = self.composer_cursor_position(width);
        let max_cursor_x = width.max(1).saturating_sub(1) as u16;
        let composer_height = layout.composer.height.max(1);
        let cursor_y = full_cursor
            .y
            .saturating_sub(layout.composer_start as u16)
            .min(composer_height.saturating_sub(1));
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
    pub(super) fn active_lines_for_height(
        &mut self,
        width: usize,
        viewport_height: usize,
    ) -> Vec<Line<'static>> {
        self.active_lines_at_for_height(width, viewport_height, Instant::now())
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
        let composer_lines = self.composer_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        let layout = self.screen_layout_for_history_len(
            area,
            history_len,
            &composer_lines,
            command_lines.len(),
        );
        let (history_start, history_count) =
            self.visible_history_window(history_len, layout.history.height as usize);
        let mut lines = self.visible_history_lines(width, now, history_start, history_count);
        lines.resize(layout.history.height as usize, Line::default());
        if let Some(activity) = layout.activity {
            lines[activity.y.saturating_sub(layout.history.y) as usize] =
                self.loading_spinner.line(
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
            lines.extend(
                self.subagent_panel
                    .lines(width, layout.subagents.height as usize),
            );
        }
        if layout.top_divider.height > 0 {
            lines.push(self.divider_line(width));
        }
        lines.extend(
            composer_lines
                .into_iter()
                .skip(layout.composer_start)
                .take(layout.composer.height as usize),
        );
        if layout.bottom_divider.height > 0 {
            lines.push(self.divider_line(width));
        }
        lines.extend(
            self.statusline_lines(width)
                .iter()
                .take(layout.statusline.height as usize)
                .cloned(),
        );
        lines.extend(
            command_lines
                .into_iter()
                .take(layout.commands.height as usize),
        );

        ActiveFrame { lines }
    }

    fn divider_line(&self, width: usize) -> Line<'static> {
        let divider_style = match &self.composer {
            ComposerMode::Input => match inline_shell::mode_when_idle(self.running, &self.input) {
                Some(InlineShellMode::IncludeInContext) => Theme::shell_context(),
                Some(InlineShellMode::ExcludeFromContext) => Theme::shell_local(),
                None => Theme::reasoning_input_border(self.info.runtime.reasoning),
            },
            ComposerMode::Picker(_)
            | ComposerMode::Questionnaire(_)
            | ComposerMode::Approval(_) => Theme::input_prompt(),
            ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::OAuthPending(_) => Theme::dim(),
        };
        Line::styled("─".repeat(width.max(1)), divider_style)
    }

    #[cfg(test)]
    pub(super) fn history_lines(&mut self, width: usize, now: Instant) -> Vec<Line<'static>> {
        let history_len = self.history_len(width, now);
        self.visible_history_lines(width, now, 0, history_len)
    }

    pub(super) fn session_header_lines(&mut self, width: usize) -> &[Line<'static>] {
        let update_notice = self.info.services.update_notice.clone();
        let stale = self
            .session_header_cache
            .as_ref()
            .is_none_or(|cache| cache.width != width || cache.update_notice != update_notice);
        if stale {
            self.session_header_cache = Some(SessionHeaderCache {
                width,
                update_notice,
                lines: session_header_lines(self.info.services.update_notice.as_deref(), width),
            });
        }
        &self.session_header_cache.as_ref().unwrap().lines
    }

    pub(super) fn history_len(&mut self, width: usize, now: Instant) -> usize {
        self.history_static_len(width)
            .saturating_add(self.history_live_lines(width, now).len())
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
            let markdown_images = &self.markdown_images;
            self.history_lines.extend_visible_lines(
                &self.transcript,
                width,
                self.info.runtime.max_tool_output_lines,
                HistoryLineSlice {
                    start: transcript_start,
                    count: transcript_count,
                },
                &mut lines,
                &|entry_index, sources| markdown_images.ready_images(entry_index, sources, &cwd),
            );
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

    pub(super) fn history_static_len(&mut self, width: usize) -> usize {
        self.session_header_lines(width)
            .len()
            .saturating_add(self.cached_transcript_line_count(width))
    }

    pub(super) fn cached_transcript_line_count(&mut self, width: usize) -> usize {
        let cwd = self.info.runtime.cwd.clone();
        let markdown_images = &self.markdown_images;
        self.history_lines.line_count(
            &self.transcript,
            width,
            self.info.runtime.max_tool_output_lines,
            &|entry_index, sources| markdown_images.ready_images(entry_index, sources, &cwd),
        )
    }

    pub(super) fn code_block_copy_targets(&mut self, width: usize) -> Vec<CodeBlockCopyTarget> {
        let header_len = self.session_header_lines(width).len();
        let cwd = self.info.runtime.cwd.clone();
        let markdown_images = &self.markdown_images;
        self.history_lines
            .code_blocks(
                &self.transcript,
                width,
                self.info.runtime.max_tool_output_lines,
                &|entry_index, sources| markdown_images.ready_images(entry_index, sources, &cwd),
            )
            .iter()
            .map(|block: &CachedCodeBlock| CodeBlockCopyTarget {
                line: header_len.saturating_add(block.line),
                columns: block.copy_columns.clone(),
                text: Arc::clone(&block.text),
            })
            .collect()
    }

    pub(super) fn history_live_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for pending in self.running_inline_shell_entries() {
            if !lines.is_empty()
                || self.last_inserted_was_tool
                || self.transcript.last().is_some_and(is_tool_entry)
            {
                lines.push(Line::raw(""));
            }
            lines.extend(tool_entry_lines(
                &pending,
                width,
                self.info.runtime.max_tool_output_lines,
            ));
        }
        if let Some(pending) = &self.pending_tool_call {
            if self.last_inserted_was_tool || self.transcript.last().is_some_and(is_tool_entry) {
                lines.push(Line::raw(""));
            }
            lines.extend(tool_entry_lines(
                pending,
                width,
                self.info.runtime.max_tool_output_lines,
            ));
        }
        if let Some(preview) = &self.live_stream_preview {
            lines.extend(self.render_stream_preview_lines(preview, width));
        }
        if self.hidden_reasoning_active {
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

    pub(super) fn visible_history_window(
        &self,
        history_len: usize,
        height: usize,
    ) -> (usize, usize) {
        let count = if self.loading_active() && matches!(self.history_scroll, HistoryScroll::Bottom)
        {
            height.saturating_sub(1)
        } else {
            height
        };
        (self.visible_history_start(history_len, count), count)
    }

    pub(super) fn visible_history_start(&self, history_len: usize, height: usize) -> usize {
        let max_start = history_len.saturating_sub(height);
        match self.history_scroll {
            HistoryScroll::Bottom => max_start,
            HistoryScroll::Manual { top_line } => top_line.min(max_start),
        }
    }

    #[cfg(test)]
    pub(super) fn should_show_jump_to_bottom(
        &mut self,
        width: usize,
        height: usize,
        now: Instant,
    ) -> bool {
        let history_len = self.history_len(width, now);
        let history_height = self.history_height_for_screen(width, height, now);
        history_height > 0
            && self.visible_history_start(history_len, history_height)
                < history_len.saturating_sub(history_height)
    }

    pub(super) fn scroll_history_to_bottom(&mut self) {
        self.history_scroll = HistoryScroll::Bottom;
        self.hide_history_scrollbar();
    }

    pub(super) fn scroll_history_page_up(&mut self, width: usize, height: usize, now: Instant) {
        let page = self.history_height_for_screen(width, height, now).max(1);
        self.scroll_history_lines(width, height, now, -(page as isize));
    }

    fn scroll_history_page_down(&mut self, width: usize, height: usize, now: Instant) {
        let page = self.history_height_for_screen(width, height, now).max(1);
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
        let composer_line_count = self.composer_lines(width).len();
        let command_line_count = self.command_suggestion_lines(width).len();
        let history_height =
            self.history_height_from_line_counts(height, composer_line_count, command_line_count);
        let max_start = history_len.saturating_sub(history_height);
        let current = self.visible_history_start(history_len, history_height);
        let next = current.saturating_add_signed(delta).min(max_start);
        self.history_scroll = scroll_state_for_top_line(history_len, history_height, next);
        if matches!(self.history_scroll, HistoryScroll::Bottom) {
            self.hide_history_scrollbar();
        }
    }

    pub(super) fn reveal_history_scrollbar(&mut self, now: Instant) {
        self.history_scrollbar_visible_until = Some(now + HISTORY_SCROLLBAR_REVEAL_DURATION);
    }

    pub(super) fn hide_history_scrollbar(&mut self) {
        self.history_scrollbar_drag = None;
        self.history_scrollbar_visible_until = None;
        self.history_scrollbar_hovered = false;
    }

    pub(super) fn should_render_history_scrollbar(&self, now: Instant) -> bool {
        self.history_scrollbar_drag.is_some()
            || self.history_scrollbar_hovered
            || self
                .history_scrollbar_visible_until
                .is_some_and(|visible_until| now < visible_until)
    }

    pub(super) fn update_history_scrollbar_hover(
        &mut self,
        scrollbar: Option<HistoryScrollbar>,
        column: u16,
        row: u16,
    ) {
        self.history_scrollbar_hovered =
            scrollbar.is_some_and(|scrollbar| scrollbar.contains(column, row));
    }

    pub(super) fn clamp_history_scroll(&mut self, width: usize, height: usize, now: Instant) {
        if matches!(self.history_scroll, HistoryScroll::Bottom) {
            self.history_scrollbar_drag = None;
            return;
        }
        let history_len = self.history_len(width, now);
        let composer_line_count = self.composer_lines(width).len();
        let command_line_count = self.command_suggestion_lines(width).len();
        let history_height =
            self.history_height_from_line_counts(height, composer_line_count, command_line_count);
        let max_start = history_len.saturating_sub(history_height);
        if let HistoryScroll::Manual { top_line } = self.history_scroll {
            self.history_scroll =
                scroll_state_for_top_line(history_len, history_height, top_line.min(max_start));
            if matches!(self.history_scroll, HistoryScroll::Bottom) {
                self.hide_history_scrollbar();
            }
        }
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
        let size = terminal.size()?;
        let width = size.width as usize;
        let height = size.height as usize;
        let now = Instant::now();
        match (key.modifiers, key.code) {
            (_, KeyCode::PageUp) => {
                self.reveal_history_scrollbar(now);
                self.history_scrollbar_drag = None;
                self.scroll_history_page_up(width, height, now);
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::PageDown) => {
                self.reveal_history_scrollbar(now);
                self.history_scrollbar_drag = None;
                self.scroll_history_page_down(width, height, now);
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ if self.info.runtime.keybindings.jump_to_bottom.matches(key) => {
                self.scroll_history_to_bottom();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn composer_lines(&self, width: usize) -> Vec<Line<'static>> {
        match &self.composer {
            ComposerMode::Input => {
                let focused_paste = self
                    .focused_paste_segment()
                    .map(|segment| segment.start..segment.end());
                let mut lines = input_lines_with_images(
                    &self.input,
                    &self.pending_images,
                    width,
                    focused_paste,
                );
                if let Some(mode) = inline_shell::mode_when_idle(self.running, &self.input) {
                    let style = match mode {
                        InlineShellMode::IncludeInContext => Theme::shell_context(),
                        InlineShellMode::ExcludeFromContext => Theme::shell_local(),
                    };
                    for line in &mut lines {
                        *line = line.clone().style(style);
                    }
                }
                lines
            }
            ComposerMode::Picker(picker) => picker_lines(picker, width),
            ComposerMode::SecretInput(secret) => secret_input_lines(secret, width),
            ComposerMode::ConfigNumberInput(input) => config_number_input_lines(input, width),
            ComposerMode::ConfigTextInput(input) => config_text_input_lines(input, width),
            ComposerMode::OAuthPending(target) => oauth_pending_lines(target, width),
            ComposerMode::Questionnaire(questionnaire) => questionnaire_lines(questionnaire, width),
            ComposerMode::Approval(approval) => approval_lines(approval, width),
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
        self.statusline.update_model(&self.info.runtime);
        self.statusline.update_usage(
            self.cumulative_usage.as_ref(),
            self.current_context.as_ref(),
        );
        self.statusline
            .update_model_metadata(self.model_metadata.as_ref());
    }

    pub(super) fn statusline_lines(&mut self, width: usize) -> &[Line<'static>] {
        let goal = self.goal_status();
        self.refresh_statusline_state();
        self.statusline.lines(width, goal)
    }

    pub(super) fn composer_cursor_position(&self, width: usize) -> Position {
        match &self.composer {
            ComposerMode::Input => {
                let mut position = input_cursor_position(&self.input, self.input_cursor, width);
                position.y = position.y.saturating_add(self.pending_images.len() as u16);
                position
            }
            ComposerMode::SecretInput(secret) => Position {
                x: char_prefix_display_width(&secret.value, secret.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigNumberInput(input) => Position {
                x: char_prefix_display_width(&input.value, input.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigTextInput(input) => Position {
                x: char_prefix_display_width(&input.value, input.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire_cursor_position(questionnaire, width)
            }
            ComposerMode::OAuthPending(_) | ComposerMode::Approval(_) => Position { x: 0, y: 0 },
            ComposerMode::Picker(picker) => Position {
                x: display_width(&picker.filter)
                    .saturating_add(2)
                    .min(width.saturating_sub(1)) as u16,
                y: 0,
            },
        }
    }

    pub(super) fn command_suggestion_lines(&self, width: usize) -> Vec<Line<'static>> {
        if let Some((text, style)) = inline_shell::mode_hint_when_idle(self.running, &self.input) {
            return vec![styled_line(
                truncate_one_line(text, width.max(1)),
                width.max(1),
                style,
                LineFill::Natural,
            )];
        }
        if self.command_palette_visible() {
            let matches = self.command_matches();
            let selected_index = self.command_selection.min(matches.len().saturating_sub(1));
            let start = selected_index
                .saturating_add(1)
                .saturating_sub(MAX_COMMAND_SUGGESTIONS);

            let usage_width = matches
                .iter()
                .skip(start)
                .take(MAX_COMMAND_SUGGESTIONS)
                .map(|command| display_width(&command.usage))
                .max()
                .unwrap_or(1)
                .min(
                    width
                        .saturating_sub(MIN_COMMAND_DESCRIPTION_WIDTH + 3)
                        .max(1),
                );

            return matches
                .into_iter()
                .enumerate()
                .skip(start)
                .take(MAX_COMMAND_SUGGESTIONS)
                .map(|(index, command)| {
                    let selected = index == selected_index;
                    let marker = if selected { ">" } else { " " };
                    let description_width = width.saturating_sub(usage_width + 3).max(1);
                    let usage = truncate_one_line(&command.usage, usage_width);
                    let description = truncate_one_line(&command.description, description_width);
                    let usage_padding =
                        " ".repeat(usage_width.saturating_sub(display_width(&usage)));
                    let text = format!("{marker} {usage}{usage_padding} {description}");
                    let style = if selected {
                        Theme::brand()
                    } else {
                        Theme::dim()
                    };
                    styled_line(text, width.max(1), style, LineFill::Natural)
                })
                .collect();
        }

        if !self.file_palette_visible() {
            return Vec::new();
        }

        let matches = self.file_matches();
        let selected_index = self.file_selection.min(matches.len().saturating_sub(1));
        let (start, above, below) = file_picker::file_palette_scroll_counts(
            matches.len(),
            selected_index,
            MAX_COMMAND_SUGGESTIONS,
        );

        let mut lines = matches
            .iter()
            .enumerate()
            .skip(start)
            .take(MAX_COMMAND_SUGGESTIONS)
            .map(|(index, path)| {
                let selected = index == selected_index;
                let marker = if selected { ">" } else { " " };
                let text = format!("{marker} @{path}");
                let style = if selected {
                    Theme::brand()
                } else {
                    Theme::dim()
                };
                styled_line(
                    truncate_one_line(&text, width.max(1)),
                    width.max(1),
                    style,
                    LineFill::Natural,
                )
            })
            .collect::<Vec<_>>();

        if let Some(footer) = file_picker::file_palette_scroll_footer(above, below, matches.len()) {
            lines.push(styled_line(
                truncate_one_line(&footer, width.max(1)),
                width.max(1),
                Theme::dim(),
                LineFill::Natural,
            ));
        }

        lines
    }

    pub(super) fn insert_session_intro(
        &self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let _ = terminal.size()?;
        Ok(())
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
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.runtime.max_tool_output_lines,
        );
        let mut transcript = Vec::new();
        if omitted > 0 {
            transcript.push(Entry::Notice(format!(
                "resumed session; showing last {} messages, omitted {omitted} earlier messages",
                visible_entries.len()
            )));
        }
        transcript.extend(visible_entries);
        self.transcript = transcript;
        self.markdown_images.clear();
        self.mark_markdown_images_dirty_from(0);
        self.history_lines.invalidate_from(0);
        self.last_status_notice = self.transcript.iter().rev().find_map(|entry| match entry {
            Entry::Notice(text) => Some(text.clone()),
            Entry::User(_)
            | Entry::Assistant(_)
            | Entry::Reasoning(_)
            | Entry::RuntimeInfo(_)
            | Entry::UsageLimits(_)
            | Entry::Tool(_)
            | Entry::Error(_) => None,
        });
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        Ok(())
    }
}
