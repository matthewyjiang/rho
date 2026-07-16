use std::time::Instant;

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{backend::Backend, layout::Rect, Terminal};

use super::{
    copy_interaction::{code_block_copy_target_at, selection_position, selection_position_clamped},
    expandable_tool_entry, is_tool_entry,
    render::tool_entry_lines,
    text_selection::{CopyNotice, TextSelection},
    tool_display_line_count, App,
};

impl App {
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
                self.text_selection = None;
                self.hovered_code_block_copy = None;
                self.reveal_history_scrollbar(now);
                self.history_scrollbar_drag = None;
                self.scroll_history_lines(width, height, now, -3);
            }
            MouseEventKind::ScrollDown => {
                self.text_selection = None;
                self.hovered_code_block_copy = None;
                self.reveal_history_scrollbar(now);
                self.history_scrollbar_drag = None;
                self.scroll_history_lines(width, height, now, 3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                let history_start =
                    self.visible_history_start(layout.history_len, layout.history.height as usize);
                let targets = self.code_block_copy_targets(width);
                let code_target =
                    code_block_copy_target_at(&targets, layout.history, history_start, column, row);
                let scrollbar = layout
                    .history_scrollbar
                    .filter(|scrollbar| scrollbar.contains(column, row))
                    .filter(|_| self.should_render_history_scrollbar(now));
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                self.hovered_code_block_copy = code_target.as_ref().map(|target| target.line);
                if let Some(scrollbar) = scrollbar {
                    self.text_selection = None;
                    self.reveal_history_scrollbar(now);
                    let drag = scrollbar.begin_drag(row);
                    self.history_scrollbar_drag = Some(drag);
                    self.history_scroll = scrollbar.scroll_state_for_pointer(row, drag);
                } else if layout.jump_to_bottom.is_some_and(|rect| {
                    rect.contains(ratatui::layout::Position { x: column, y: row })
                }) {
                    self.text_selection = None;
                    self.history_scrollbar_drag = None;
                    self.scroll_history_to_bottom();
                } else if let Some(target) = code_target {
                    self.text_selection = None;
                    self.copy_text(&target.text, now);
                } else if let Some(position) =
                    selection_position(layout.history, history_start, column, row)
                {
                    self.history_scrollbar_drag = None;
                    self.text_selection = Some(TextSelection::new(position));
                } else {
                    self.text_selection = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                if let Some(drag) = self.history_scrollbar_drag {
                    self.text_selection = None;
                    self.hovered_code_block_copy = None;
                    if let Some(scrollbar) = layout.history_scrollbar {
                        self.history_scroll = scrollbar.scroll_state_for_pointer(row, drag);
                    }
                } else {
                    let history_start = self
                        .visible_history_start(layout.history_len, layout.history.height as usize);
                    let targets = self.code_block_copy_targets(width);
                    self.hovered_code_block_copy = code_block_copy_target_at(
                        &targets,
                        layout.history,
                        history_start,
                        column,
                        row,
                    )
                    .map(|target| target.line);
                    if let (Some(selection), Some(position)) = (
                        &mut self.text_selection,
                        selection_position_clamped(layout.history, history_start, column, row),
                    ) {
                        selection.update(position);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let was_scrollbar_drag = self.history_scrollbar_drag.take().is_some();
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                let history_start =
                    self.visible_history_start(layout.history_len, layout.history.height as usize);
                let targets = self.code_block_copy_targets(width);
                self.hovered_code_block_copy =
                    code_block_copy_target_at(&targets, layout.history, history_start, column, row)
                        .map(|target| target.line);
                if was_scrollbar_drag {
                    self.text_selection = None;
                } else if let Some(mut selection) = self.text_selection.take() {
                    let release_position =
                        selection_position_clamped(layout.history, history_start, column, row);
                    if let Some(position) = release_position {
                        selection.update(position);
                    }
                    if selection.has_moved() {
                        let visible_lines = self.visible_history_lines(
                            width,
                            now,
                            history_start,
                            layout.history.height as usize,
                        );
                        if let Some(text) = selection.selected_text(&visible_lines, history_start) {
                            self.copy_text(&text, now);
                            self.text_selection = Some(selection);
                        }
                    } else if release_position.is_some() {
                        let line = history_start
                            .saturating_add(row.saturating_sub(layout.history.y) as usize);
                        self.toggle_tool_output_at_history_line(line, width, terminal)?;
                    }
                }
            }
            MouseEventKind::Moved if self.last_mouse_position == Some((column, row)) => {}
            MouseEventKind::Moved => {
                self.last_mouse_position = Some((column, row));
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                let history_start =
                    self.visible_history_start(layout.history_len, layout.history.height as usize);
                self.hovered_code_block_copy = if layout
                    .history
                    .contains(ratatui::layout::Position { x: column, y: row })
                {
                    let targets = self.code_block_copy_targets(width);
                    code_block_copy_target_at(&targets, layout.history, history_start, column, row)
                        .map(|target| target.line)
                } else {
                    None
                };
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
            let index = self.history_lines.entry_index_at_line(
                &self.transcript,
                width,
                self.info.max_tool_output_lines,
                transcript_line,
            );
            if let Some(index) = index.filter(|&index| {
                self.transcript.get(index).is_some_and(|entry| {
                    expandable_tool_entry(entry, self.info.max_tool_output_lines)
                })
            }) {
                self.toggle_transcript_tool_output(index);
                self.clamp_history_scroll_for_terminal(terminal)?;
                return Ok(true);
            }
        }

        let static_len = self.history_static_len(width);
        let mut pending_start = static_len;
        let transcript_ends_with_tool =
            self.last_inserted_was_tool || self.transcript.last().is_some_and(is_tool_entry);
        for (shell_index, shell) in self.running_inline_shell_entries().enumerate() {
            if shell_index > 0 || transcript_ends_with_tool {
                pending_start = pending_start.saturating_add(1);
            }
            pending_start = pending_start.saturating_add(
                tool_entry_lines(&shell, width, self.info.max_tool_output_lines).len(),
            );
        }
        if let Some(pending) = self.pending_tool_call.as_ref() {
            if transcript_ends_with_tool {
                pending_start = pending_start.saturating_add(1);
            }
            let pending_end = pending_start.saturating_add(
                tool_entry_lines(pending, width, self.info.max_tool_output_lines).len(),
            );
            if (pending_start..pending_end).contains(&line)
                && tool_display_line_count(&pending.display_lines) > self.info.max_tool_output_lines
            {
                let pending = self
                    .pending_tool_call
                    .as_mut()
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
        }

        Ok(false)
    }

    fn copy_text(&mut self, text: &str, now: Instant) {
        self.copy_notice = Some(match self.clipboard.copy(text) {
            Ok(()) => CopyNotice::copied(text.chars().count(), now),
            Err(error) => CopyNotice::failed(&error, now),
        });
    }
}
