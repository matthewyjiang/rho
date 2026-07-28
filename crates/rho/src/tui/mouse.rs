use std::time::Instant;

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{backend::Backend, layout::Rect, Terminal};

use super::{
    copy_interaction::{code_block_copy_target_at, selection_position, selection_position_clamped},
    render::tool_entry_lines,
    text_selection::{screen_lines, CopyNotice, TextSelection},
    tool_output_ui::{expandable_tool_entry, tool_output_toggleable},
    App, ComposerMode,
};

impl App {
    /// Drops both the history-anchored and screen-space text selections.
    pub(super) fn clear_selections(&mut self) {
        self.history.clear_text_selection();
        self.screen_selection = None;
    }

    fn mouse_history_view(&self, history_content: Rect, history_len: usize) -> (Rect, usize) {
        let (history_start, _) =
            self.visible_history_window(history_len, history_content.height as usize);
        (history_content, history_start)
    }

    pub(super) fn handle_mouse_event<B: Backend>(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        terminal: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let size = terminal.size()?;
        let width = size.width as usize;
        let height = size.height as usize;
        let now = Instant::now();
        match kind {
            MouseEventKind::ScrollUp => {
                self.screen_selection = None;
                self.history.set_hovered_code_block_copy(None);
                self.subagent_panel.clear_pointer_state();
                self.reveal_history_scrollbar(now);
                self.history.set_scrollbar_drag(None);
                self.scroll_history_lines(
                    width,
                    height,
                    now,
                    -(super::HISTORY_MOUSE_SCROLL_LINES as isize),
                );
            }
            MouseEventKind::ScrollDown => {
                self.screen_selection = None;
                self.history.set_hovered_code_block_copy(None);
                self.subagent_panel.clear_pointer_state();
                self.reveal_history_scrollbar(now);
                self.history.set_scrollbar_drag(None);
                self.scroll_history_lines(
                    width,
                    height,
                    now,
                    super::HISTORY_MOUSE_SCROLL_LINES as isize,
                );
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.screen_selection = None;
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                let (history, history_start) =
                    self.mouse_history_view(layout.history_content, layout.history_len);
                let targets = self.code_block_copy_targets(width);
                let code_target =
                    code_block_copy_target_at(&targets, history, history_start, column, row);
                let scrollbar = layout
                    .history_scrollbar
                    .filter(|scrollbar| scrollbar.contains(column, row))
                    .filter(|_| self.should_render_history_scrollbar(now));
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                self.history
                    .set_hovered_code_block_copy(code_target.as_ref().map(|target| target.line));
                let subagent_target = matches!(self.input_ui.composer(), ComposerMode::Input)
                    .then(|| {
                        self.subagent_panel
                            .attach_target_at(layout.subagents, column, row)
                    })
                    .flatten();
                if let Some(target) = subagent_target {
                    self.history.clear_text_selection();
                    self.history.set_scrollbar_drag(None);
                    self.subagent_panel.set_pressed(Some(&target.run_id));
                    self.subagent_panel.set_hovered(Some(&target.run_id));
                } else if let Some(scrollbar) = scrollbar {
                    self.subagent_panel.clear_pointer_state();
                    self.history.clear_text_selection();
                    self.history.scroll_chrome_mut().begin_scrollbar_drag(
                        scrollbar,
                        row,
                        now,
                        super::HISTORY_SCROLLBAR_REVEAL_DURATION,
                    );
                } else if layout.jump_to_bottom.is_some_and(|rect| {
                    rect.contains(ratatui::layout::Position { x: column, y: row })
                }) {
                    self.subagent_panel.clear_pointer_state();
                    self.history.clear_text_selection();
                    self.history.set_scrollbar_drag(None);
                    self.scroll_history_to_bottom();
                } else if let Some(target) = code_target {
                    self.subagent_panel.clear_pointer_state();
                    self.history.clear_text_selection();
                    self.copy_text(&target.text, now);
                } else if let Some(position) =
                    selection_position(history, history_start, column, row)
                {
                    self.subagent_panel.clear_pointer_state();
                    self.history.set_scrollbar_drag(None);
                    *self.history.text_selection_mut() = Some(TextSelection::new(position));
                } else {
                    self.subagent_panel.clear_pointer_state();
                    self.history.clear_text_selection();
                    let screen = Rect::new(0, 0, size.width, size.height);
                    self.screen_selection =
                        selection_position_clamped(screen, 0, column, row).map(TextSelection::new);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                self.subagent_panel.clear_pointer_state();
                if self.history.scrollbar_drag().is_some() {
                    self.history.clear_text_selection();
                    self.history.set_hovered_code_block_copy(None);
                    if let Some(scrollbar) = layout.history_scrollbar {
                        self.history.scroll_chrome_mut().drag_to(scrollbar, row);
                    }
                } else if self.screen_selection.is_some() {
                    let screen = Rect::new(0, 0, size.width, size.height);
                    if let (Some(selection), Some(position)) = (
                        self.screen_selection.as_mut(),
                        selection_position_clamped(screen, 0, column, row),
                    ) {
                        selection.update(position);
                    }
                } else {
                    let (history, history_start) =
                        self.mouse_history_view(layout.history_content, layout.history_len);
                    let targets = self.code_block_copy_targets(width);
                    self.history.set_hovered_code_block_copy(
                        code_block_copy_target_at(&targets, history, history_start, column, row)
                            .map(|target| target.line),
                    );
                    if let (Some(selection), Some(position)) = (
                        self.history.text_selection_mut().as_mut(),
                        selection_position_clamped(history, history_start, column, row),
                    ) {
                        selection.update(position);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let pressed_subagent = self.subagent_panel.pressed_run_id().map(str::to_owned);
                let was_scrollbar_drag = self.history.scrollbar_drag().is_some();
                self.history.set_scrollbar_drag(None);
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                let released_subagent = matches!(self.input_ui.composer(), ComposerMode::Input)
                    .then(|| {
                        self.subagent_panel
                            .attach_target_at(layout.subagents, column, row)
                    })
                    .flatten();
                self.subagent_panel.set_pressed(None);
                self.subagent_panel.set_hovered(
                    released_subagent
                        .as_ref()
                        .map(|target| target.run_id.as_str()),
                );
                let activate_subagent = released_subagent
                    .filter(|target| pressed_subagent.as_deref() == Some(target.run_id.as_str()));
                let (history, history_start) =
                    self.mouse_history_view(layout.history_content, layout.history_len);
                let targets = self.code_block_copy_targets(width);
                self.history.set_hovered_code_block_copy(
                    code_block_copy_target_at(&targets, history, history_start, column, row)
                        .map(|target| target.line),
                );
                if let Some(target) = activate_subagent {
                    self.history.clear_text_selection();
                    self.activate_subagent_row(&target, now);
                } else if was_scrollbar_drag {
                    self.history.clear_text_selection();
                } else if let Some(mut selection) = self.history.text_selection_mut().take() {
                    let release_position =
                        selection_position_clamped(history, history_start, column, row);
                    if let Some(position) = release_position {
                        selection.update(position);
                    }
                    if selection.has_moved() {
                        let selected_lines = selection.selected_line_range();
                        let lines = self.visible_history_lines(
                            width,
                            now,
                            selected_lines.start,
                            selected_lines.len(),
                        );
                        if let Some(text) = selection.selected_text(&lines, selected_lines.start) {
                            self.copy_text(&text, now);
                            *self.history.text_selection_mut() = Some(selection);
                        }
                    } else if release_position.is_some() {
                        let line =
                            history_start.saturating_add(row.saturating_sub(history.y) as usize);
                        self.toggle_tool_output_at_history_line(line, width, terminal)?;
                    }
                } else if let Some(mut selection) = self.screen_selection.take() {
                    let screen = Rect::new(0, 0, size.width, size.height);
                    if let Some(position) = selection_position_clamped(screen, 0, column, row) {
                        selection.update(position);
                    }
                    if selection.has_moved() {
                        // Redraw so the completed frame holds the text the
                        // selection was made over; the terminal's current
                        // buffer is the cleared back buffer after a draw.
                        let completed = terminal.draw(|frame| self.draw(frame))?;
                        let lines = screen_lines(completed.buffer, screen);
                        if let Some(text) = selection.selected_text(&lines, 0) {
                            self.copy_text(&text, now);
                            self.screen_selection = Some(selection);
                        }
                    }
                }
            }
            MouseEventKind::Moved if self.last_mouse_position == Some((column, row)) => {}
            MouseEventKind::Moved => {
                self.last_mouse_position = Some((column, row));
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                let (history, history_start) =
                    self.mouse_history_view(layout.history_content, layout.history_len);
                let hovered = if history.contains(ratatui::layout::Position { x: column, y: row }) {
                    let targets = self.code_block_copy_targets(width);
                    code_block_copy_target_at(&targets, history, history_start, column, row)
                        .map(|target| target.line)
                } else {
                    None
                };
                self.history.set_hovered_code_block_copy(hovered);
                let subagent_hover = matches!(self.input_ui.composer(), ComposerMode::Input)
                    .then(|| {
                        self.subagent_panel
                            .attach_target_at(layout.subagents, column, row)
                            .map(|target| target.run_id)
                    })
                    .flatten();
                self.subagent_panel.set_hovered(subagent_hover.as_deref());
            }
            MouseEventKind::Down(MouseButton::Right)
            | MouseEventKind::Down(MouseButton::Middle)
            | MouseEventKind::Up(MouseButton::Right)
            | MouseEventKind::Up(MouseButton::Middle)
            | MouseEventKind::Drag(MouseButton::Right)
            | MouseEventKind::Drag(MouseButton::Middle)
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {}
        }
        Ok(())
    }

    fn toggle_tool_output_at_history_line<B: Backend>(
        &mut self,
        line: usize,
        width: usize,
        terminal: &mut Terminal<B>,
    ) -> Result<bool, B::Error> {
        let header_len = self.session_header_lines(width).len();
        if let Some(transcript_line) = line.checked_sub(header_len) {
            let cwd = self.info.runtime.cwd.clone();
            let max_tool_output_lines = self.info.runtime.max_tool_output_lines;
            self.sync_open_stream_tail();
            let index = self.history.with_lines_and_images_mut(
                |history_lines, entries, markdown_images| {
                    history_lines.entry_index_at_line(
                        entries,
                        width,
                        max_tool_output_lines,
                        transcript_line,
                        &|entry_index, sources| {
                            markdown_images.ready_images(entry_index, sources, &cwd)
                        },
                    )
                },
            );
            if let Some(index) = index.filter(|&index| {
                self.history.get(index).is_some_and(|entry| {
                    expandable_tool_entry(entry, self.info.runtime.max_tool_output_lines, width)
                })
            }) {
                self.toggle_transcript_tool_output(index);
                self.clamp_history_scroll_for_terminal(terminal)?;
                return Ok(true);
            }
        }

        let static_len = self.history_static_len(width);
        let mut pending_start = static_len;
        let shells = self.running_inline_shell_entries().collect::<Vec<_>>();
        let has_pending_tools =
            !shells.is_empty() || self.turn.tool_calls().live_entries().next().is_some();
        // Match history_live_lines: open stream tails need one blank before live tools.
        if has_pending_tools && self.open_stream_tail_active() {
            pending_start = pending_start.saturating_add(1);
        }
        for shell in &shells {
            pending_start = pending_start.saturating_add(
                tool_entry_lines(shell, width, self.info.runtime.max_tool_output_lines).len(),
            );
        }
        enum PendingToolKey {
            Preview(usize),
            Running(rho_sdk::ToolCallId),
        }
        let mut target = None;
        let entries = self
            .turn
            .tool_calls()
            .previews
            .iter()
            .map(|(index, entry)| (PendingToolKey::Preview(*index), entry))
            .chain(
                self.turn
                    .tool_calls()
                    .running
                    .iter()
                    .map(|(call_id, entry)| (PendingToolKey::Running(call_id.clone()), entry)),
            );
        for (key, pending) in entries {
            let pending_end = pending_start.saturating_add(
                tool_entry_lines(pending, width, self.info.runtime.max_tool_output_lines).len(),
            );
            if (pending_start..pending_end).contains(&line)
                && tool_output_toggleable(pending, self.info.runtime.max_tool_output_lines, width)
            {
                target = Some(key);
                break;
            }
            pending_start = pending_end;
        }
        if let Some(target) = target {
            let pending = match target {
                PendingToolKey::Preview(index) => {
                    self.turn.tool_calls_mut().previews.get_mut(&index)
                }
                PendingToolKey::Running(call_id) => {
                    self.turn.tool_calls_mut().running.get_mut(&call_id)
                }
            }
            .expect("pending tool exists");
            pending.expanded = !pending.expanded;
            self.status = if pending.expanded {
                "tool output expanded".into()
            } else {
                "tool output collapsed".into()
            };
            self.clamp_history_scroll_for_terminal(terminal)?;
            return Ok(true);
        }

        Ok(false)
    }

    pub(super) fn copy_text(&mut self, text: &str, now: Instant) {
        let character_count = text.chars().count();
        self.history
            .set_copy_notice(Some(CopyNotice::from_copy_result(
                self.clipboard.copy(text),
                character_count,
                now,
            )));
    }
}
