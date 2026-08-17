use std::ops::Range;
use std::time::Instant;

use ratatui::{
    layout::{Position, Rect},
    style::Style,
    text::Line,
    widgets::{Clear, Paragraph},
    DefaultTerminal, Frame,
};

use super::tool_call_batch::LiveToolKey;
use super::tool_card_hover::ToolCardTarget;
use super::tool_output_ui::tool_output_toggleable;
use super::{
    composer_chrome::ComposerDividerSlot,
    highlight_selection,
    picker_overlay::picker_overlay_frame,
    render::{pad_display_line, padded_content_width, truncate_one_line},
    render_copy_notice,
    screen_layout::{terminal_meets_minimum, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH},
    session_header_lines, styled_line, tool_card_hover, tool_entry_lines,
};
use super::{
    history_cache::{HistoryLineSlice, HistoryRenderSettings},
    App, CodeBlockCopyTarget, ComposerMode, Entry, GoalStatus, LineFill, ReasoningChrome,
    SessionHeaderCache, StreamKind, Theme,
};
#[cfg(test)]
use super::{ActiveFrame, DEFAULT_TUI_HEIGHT};

/// Live history paint output: lines plus toggleable card spans in that walk.
pub(super) struct LiveHistory {
    pub(super) lines: Vec<Line<'static>>,
    /// Ranges are relative to the start of `lines`.
    cards: Vec<(ToolCardTarget, Range<usize>)>,
}

impl LiveHistory {
    pub(super) fn card_hit_at(&self, live_line: usize) -> Option<(ToolCardTarget, Range<usize>)> {
        self.cards
            .iter()
            .find(|(_, range)| range.contains(&live_line))
            .map(|(target, range)| (target.clone(), range.clone()))
    }
}

impl From<LiveToolKey> for ToolCardTarget {
    fn from(key: LiveToolKey) -> Self {
        match key {
            LiveToolKey::Preview(index) => Self::Preview(index),
            LiveToolKey::Running(call_id) => Self::Running(call_id),
        }
    }
}

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
        frame.render_widget(
            ratatui::widgets::Block::default().style(Theme::surface()),
            area,
        );
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
        let height = area.height as usize;
        self.note_terminal_geometry(width, height);
        self.refresh_composer_attachment_layout_cache(width);
        let composer_lines = self.composer_lines(width, height);
        let command_lines = self.command_suggestion_lines(width);
        // One settings/budget computation feeds every history read this frame;
        // recomputing it per read re-wrapped the composer several times a frame.
        let settings = self.history_render_settings_for_chrome(
            width,
            composer_lines.len(),
            command_lines.len(),
        );
        let live_history = self.live_history_layout(width, settings.max_image_height);
        let viewport = self.history_content_height_for_screen(width, height, now);
        self.ensure_measured_history_suffix(settings, viewport);
        let history_len = self
            .history_static_len_with_settings(width, settings)
            .saturating_add(live_history.lines.len());
        let layout = self.screen_layout_for_history_len(
            area,
            history_len,
            &composer_lines,
            command_lines.len(),
        );
        let (history_start, history_count) =
            self.visible_history_window(history_len, layout.history_content.height as usize);
        let surface = DrawSurface {
            area,
            width,
            now,
            layout: &layout,
        };
        self.draw_history(
            frame,
            settings,
            surface,
            HistoryLineSlice {
                start: history_start,
                count: history_count,
            },
            &live_history,
        );
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
        settings: HistoryRenderSettings,
        surface: DrawSurface<'_>,
        slice: HistoryLineSlice,
        live_history: &LiveHistory,
    ) {
        let DrawSurface {
            width, now, layout, ..
        } = surface;
        let HistoryLineSlice {
            start: history_start,
            count: history_count,
        } = slice;
        let history_visible = self.visible_history_lines_with_live(
            width,
            settings,
            history_start,
            history_count,
            &live_history.lines,
        );
        let visible_images =
            self.visible_history_image_placements(width, settings, history_start, history_count);
        frame.render_widget(
            Paragraph::new(history_visible).style(Style::default()),
            layout.history_content,
        );
        // Hover lift derives from the remembered pointer cell against this
        // frame's layout, so scroll, streaming appends, and toggles re-anchor
        // it every draw instead of caching stale absolute lines.
        // Hover lift paints under text selection: an active drag keeps its
        // reverse-video highlight on overlapping rows.
        if let Some(lines) = self
            .last_mouse_position
            .filter(|position| {
                layout.history_content.contains((*position).into())
                    && !layout.history_scrollbar.is_some_and(|scrollbar| {
                        scrollbar.contains(position.0, position.1)
                            && self.should_render_history_scrollbar(now)
                    })
            })
            .and_then(|(_, row)| {
                let line = history_start + usize::from(row - layout.history_content.y);
                self.tool_card_hit_at_history_line(line, width, live_history)
                    .map(|hit| hit.lines)
            })
        {
            tool_card_hover::lift_lines(
                frame.buffer_mut(),
                layout.history_content,
                history_start,
                lines,
            );
        }
        if let Some(selection) = self.history.text_selection() {
            highlight_selection(
                frame.buffer_mut(),
                layout.history_content,
                history_start,
                selection,
            );
        }
        if let Some(hovered_line) = self
            .history
            .hovered_code_block_copy()
            .filter(|line| (history_start..history_start + history_count).contains(line))
        {
            if let Some(target) =
                self.code_block_copy_target_at_line_with_settings(width, settings, hovered_line)
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
            frame.render_widget(Paragraph::new("").style(Theme::surface()), activity_gap);
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
                    /*continues_below*/ layout.processes.height > 0,
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
        self.render_process_rail(frame, layout.processes, width);
        if layout.top_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width, ComposerDividerSlot::Top)])
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
        self.render_composer_images(frame, layout.composer, width, layout.composer_start);
        if layout.bottom_divider.height > 0 {
            frame.render_widget(
                Paragraph::new(vec![self.divider_line(width, ComposerDividerSlot::Bottom)])
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
            now,
        } = surface;
        let popup_cursor = match self.input_ui.composer() {
            ComposerMode::Picker(picker) => picker_overlay_frame(picker, area).map(|overlay| {
                // Clear punches host defaults; fixed themes must repaint their surface
                // or light schemes leave dark holes under dark body ink.
                frame.render_widget(Clear, overlay.outer);
                frame.render_widget(
                    Paragraph::new(overlay.lines).style(Theme::surface()),
                    overlay.outer,
                );
                overlay.cursor
            }),
            ComposerMode::Limits(_) => self.limits_overlay_frame(area, now).map(|overlay| {
                frame.render_widget(Clear, overlay.outer);
                frame.render_widget(
                    Paragraph::new(overlay.lines).style(Theme::surface()),
                    overlay.outer,
                );
                overlay.cursor
            }),
            _ => None,
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
                /*continues_below*/ layout.processes.height > 0,
            ));
        }
        lines.extend(
            self.process_panel
                .lines(width, layout.processes.height as usize),
        );
        if layout.top_divider.height > 0 {
            lines.push(self.divider_line(width, ComposerDividerSlot::Top));
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
            lines.push(self.divider_line(width, ComposerDividerSlot::Bottom));
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
        let theme_generation = Theme::generation();
        let stale = self.history.session_header_cache().is_none_or(|cache| {
            cache.width != width
                || cache.update_notice != update_notice
                || cache.setup != setup
                || cache.theme_generation != theme_generation
        });
        if stale {
            self.history
                .set_session_header_cache(Some(SessionHeaderCache {
                    width,
                    update_notice,
                    setup,
                    theme_generation,
                    lines: session_header_lines(
                        self.info.services.update_notice.as_deref(),
                        setup,
                        width,
                    ),
                }));
        }
        &self.history.session_header_cache().unwrap().lines
    }

    /// Session intro rows in the scroll document. Hidden until the unmeasured
    /// prefix is gone so resume does not glue tips to the measured tail.
    pub(super) fn visible_session_header_len(&mut self, width: usize) -> usize {
        if self.history.has_unmeasured_prefix() {
            0
        } else {
            self.session_header_lines(width).len()
        }
    }

    pub(super) fn history_len(&mut self, width: usize, now: Instant) -> usize {
        let live = self.history_live_lines(width, now);
        self.history_len_with_live(width, &live)
    }

    fn history_len_with_live(&mut self, width: usize, live: &[Line<'static>]) -> usize {
        self.history_static_len(width).saturating_add(live.len())
    }

    /// History layout inputs, including the current feed-image row budget.
    pub(super) fn history_render_settings(&self, width: usize) -> HistoryRenderSettings {
        self.info
            .runtime
            .history_render_settings(width, self.feed_image_row_budget(width))
    }

    /// History render settings from already-counted bottom chrome lines, so a
    /// frame reuses its composer render instead of re-wrapping it per read.
    pub(super) fn history_render_settings_for_chrome(
        &self,
        width: usize,
        composer_line_count: usize,
        command_line_count: usize,
    ) -> HistoryRenderSettings {
        let height = self.terminal_height;
        let content_height = self.history_content_height_from_counts(
            height,
            composer_line_count,
            command_line_count,
        );
        let budget = super::feed_image::ImageRowBudget::feed(height, content_height).get();
        self.info.runtime.history_render_settings(width, budget)
    }

    /// Record terminal size for discrete feed-image budgets and layout caches.
    pub(super) fn note_terminal_geometry(&mut self, width: usize, terminal_height: usize) {
        let _ = width;
        if terminal_height > 0 {
            self.terminal_height = terminal_height;
        }
    }

    /// Feed-image row budget: preferred terminal-height band, capped by the live
    /// history content viewport so composer chrome cannot make placements
    /// permanently unpaintable.
    pub(super) fn feed_image_row_budget(&self, width: usize) -> u16 {
        let height = self.terminal_height;
        let content_height = self.history_content_height_from_counts(
            height,
            self.composer_lines(width, height).len(),
            self.command_suggestion_lines(width).len(),
        );
        super::feed_image::ImageRowBudget::feed(height, content_height).get()
    }

    fn history_content_height_from_counts(
        &self,
        height: usize,
        composer_line_count: usize,
        command_line_count: usize,
    ) -> usize {
        if height == 0 {
            return 0;
        }
        self.history_content_height(self.history_height_from_line_counts(
            height,
            composer_line_count,
            command_line_count,
        ))
    }

    pub(super) fn visible_history_lines(
        &mut self,
        width: usize,
        now: Instant,
        start: usize,
        count: usize,
    ) -> Vec<Line<'static>> {
        let live = self.history_live_lines(width, now);
        let settings = self.history_render_settings(width);
        self.visible_history_lines_with_live(width, settings, start, count, &live)
    }

    fn visible_history_lines_with_live(
        &mut self,
        width: usize,
        settings: HistoryRenderSettings,
        start: usize,
        count: usize,
        live: &[Line<'static>],
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if count == 0 {
            return lines;
        }

        let header_len = self.visible_session_header_len(width);
        if start < header_len {
            let header_count = count.min(header_len - start);
            lines.extend(
                self.session_header_lines(width)[start..start + header_count]
                    .iter()
                    .cloned(),
            );
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
                        settings,
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

        let static_len =
            header_len.saturating_add(self.cached_transcript_line_count_with_settings(settings));
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
        let settings = self.history_render_settings(width);
        self.history_static_len_with_settings(width, settings)
    }

    fn history_static_len_with_settings(
        &mut self,
        width: usize,
        settings: HistoryRenderSettings,
    ) -> usize {
        self.visible_session_header_len(width)
            .saturating_add(self.cached_transcript_line_count_with_settings(settings))
    }

    fn cached_transcript_line_count_with_settings(
        &mut self,
        settings: HistoryRenderSettings,
    ) -> usize {
        self.sync_open_stream_tail();
        let cwd = self.info.runtime.cwd.clone();
        self.history
            .with_lines_and_images_mut(|history_lines, entries, markdown_images| {
                history_lines.line_count(entries, settings, &|entry_index, sources| {
                    markdown_images.ready_images(entry_index, sources, &cwd)
                })
            })
    }

    /// Code-block copy button whose header row sits at absolute history `line`.
    ///
    /// Hits the history-cache projection, so pointer events on long transcripts
    /// do not rebuild a target list per event.
    pub(super) fn code_block_copy_target_at_line(
        &mut self,
        width: usize,
        line: usize,
    ) -> Option<CodeBlockCopyTarget> {
        let settings = self.history_render_settings(width);
        self.code_block_copy_target_at_line_with_settings(width, settings, line)
    }

    fn code_block_copy_target_at_line_with_settings(
        &mut self,
        width: usize,
        settings: HistoryRenderSettings,
        line: usize,
    ) -> Option<CodeBlockCopyTarget> {
        self.sync_open_stream_tail();
        let header_len = self.visible_session_header_len(width);
        let transcript_line = line.checked_sub(header_len)?;
        let cwd = self.info.runtime.cwd.clone();
        self.history
            .with_lines_and_images_mut(|history_lines, entries, markdown_images| {
                let block = history_lines.code_block_at_line(
                    entries,
                    settings,
                    transcript_line,
                    &|entry_index, sources| {
                        markdown_images.ready_images(entry_index, sources, &cwd)
                    },
                )?;
                Some(CodeBlockCopyTarget {
                    line,
                    columns: block.copy_columns,
                    text: block.text,
                })
            })
    }

    /// Hit-test a pointer position against code-block copy buttons.
    pub(super) fn code_block_copy_target_at_position(
        &mut self,
        width: usize,
        history: Rect,
        history_start: usize,
        position: Position,
    ) -> Option<CodeBlockCopyTarget> {
        if !history.contains(position) {
            return None;
        }
        let line = history_start.saturating_add(position.y.saturating_sub(history.y) as usize);
        let relative_column = position.x.saturating_sub(history.x) as usize;
        self.code_block_copy_target_at_line(width, line)
            .filter(|target| target.columns.contains(&relative_column))
    }

    pub(super) fn history_live_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        self.live_history_layout(width, self.feed_image_row_budget(width))
            .lines
    }

    /// Live feed lines plus the clickable card spans in the same walk that paints them.
    pub(super) fn live_history_layout(&self, width: usize, max_image_height: u16) -> LiveHistory {
        let mut lines = Vec::new();
        let mut cards = Vec::new();
        let show_tools = self.info.runtime.shows_work_chrome();
        let shells = if show_tools {
            self.running_inline_shell_entries().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let tools = if show_tools {
            self.turn.tool_calls().live_cards().collect::<Vec<_>>()
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
                max_image_height,
            ));
        }
        let max_tool_output_lines = self.info.runtime.max_tool_output_lines;
        for (key, pending) in tools {
            let start = lines.len();
            lines.extend(tool_entry_lines(
                pending,
                width,
                max_tool_output_lines,
                max_image_height,
            ));
            let end = lines.len();
            if tool_output_toggleable(pending, max_tool_output_lines, width) {
                cards.push((ToolCardTarget::from(key), start..end));
            }
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
        LiveHistory { lines, cards }
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
    ) -> std::io::Result<bool> {
        let messages = std::mem::take(&mut self.info.session.recovered_messages);
        let had_recovered_messages = !messages.is_empty();
        let entries = self.transcript_entries(&messages);
        if entries.is_empty() {
            return Ok(had_recovered_messages);
        }

        let size = terminal.size()?;
        self.note_terminal_geometry(size.width as usize, size.height as usize);
        self.set_history_entries(entries);
        Ok(had_recovered_messages)
    }
}
