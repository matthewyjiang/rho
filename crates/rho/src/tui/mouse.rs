use std::time::Instant;

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{backend::Backend, layout::Rect, Terminal};

use super::{
    copy_interaction::{code_block_copy_target_at, selection_position, selection_position_clamped},
    render::tool_entry_lines,
    text_selection::{CopyNotice, TextSelection},
    tool_output_ui::{expandable_tool_entry, is_tool_entry, tool_display_line_count},
    App,
};

impl App {
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
                self.history.hovered_code_block_copy = None;
                self.reveal_history_scrollbar(now);
                self.history.history_scrollbar_drag = None;
                self.scroll_history_lines(width, height, now, -3);
            }
            MouseEventKind::ScrollDown => {
                self.history.hovered_code_block_copy = None;
                self.reveal_history_scrollbar(now);
                self.history.history_scrollbar_drag = None;
                self.scroll_history_lines(width, height, now, 3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
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
                self.history.hovered_code_block_copy =
                    code_target.as_ref().map(|target| target.line);
                if let Some(scrollbar) = scrollbar {
                    self.history.text_selection = None;
                    self.reveal_history_scrollbar(now);
                    let drag = scrollbar.begin_drag(row);
                    self.history.history_scrollbar_drag = Some(drag);
                    self.history.history_scroll = scrollbar.scroll_state_for_pointer(row, drag);
                } else if layout.jump_to_bottom.is_some_and(|rect| {
                    rect.contains(ratatui::layout::Position { x: column, y: row })
                }) {
                    self.history.text_selection = None;
                    self.history.history_scrollbar_drag = None;
                    self.scroll_history_to_bottom();
                } else if let Some(target) = code_target {
                    self.history.text_selection = None;
                    self.copy_text(&target.text, now);
                } else if let Some(position) =
                    selection_position(history, history_start, column, row)
                {
                    self.history.history_scrollbar_drag = None;
                    self.history.text_selection = Some(TextSelection::new(position));
                } else {
                    self.history.text_selection = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                if let Some(drag) = self.history.history_scrollbar_drag {
                    self.history.text_selection = None;
                    self.history.hovered_code_block_copy = None;
                    if let Some(scrollbar) = layout.history_scrollbar {
                        self.history.history_scroll = scrollbar.scroll_state_for_pointer(row, drag);
                    }
                } else {
                    let (history, history_start) =
                        self.mouse_history_view(layout.history_content, layout.history_len);
                    let targets = self.code_block_copy_targets(width);
                    self.history.hovered_code_block_copy =
                        code_block_copy_target_at(&targets, history, history_start, column, row)
                            .map(|target| target.line);
                    if let (Some(selection), Some(position)) = (
                        &mut self.history.text_selection,
                        selection_position_clamped(history, history_start, column, row),
                    ) {
                        selection.update(position);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let was_scrollbar_drag = self.history.history_scrollbar_drag.take().is_some();
                let layout = self.screen_layout(Rect::new(0, 0, size.width, size.height), now);
                self.update_history_scrollbar_hover(layout.history_scrollbar, column, row);
                let (history, history_start) =
                    self.mouse_history_view(layout.history_content, layout.history_len);
                let targets = self.code_block_copy_targets(width);
                self.history.hovered_code_block_copy =
                    code_block_copy_target_at(&targets, history, history_start, column, row)
                        .map(|target| target.line);
                if was_scrollbar_drag {
                    self.history.text_selection = None;
                } else if let Some(mut selection) = self.history.text_selection.take() {
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
                            self.history.text_selection = Some(selection);
                        }
                    } else if release_position.is_some() {
                        let line =
                            history_start.saturating_add(row.saturating_sub(history.y) as usize);
                        self.toggle_tool_output_at_history_line(line, width, terminal)?;
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
                self.history.hovered_code_block_copy =
                    if history.contains(ratatui::layout::Position { x: column, y: row }) {
                        let targets = self.code_block_copy_targets(width);
                        code_block_copy_target_at(&targets, history, history_start, column, row)
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
            let cwd = self.info.runtime.cwd.clone();
            let markdown_images = &self.history.markdown_images;
            let index = self.history.history_lines.entry_index_at_line(
                &self.history.transcript,
                width,
                self.info.runtime.max_tool_output_lines,
                transcript_line,
                &|entry_index, sources| markdown_images.ready_images(entry_index, sources, &cwd),
            );
            if let Some(index) = index.filter(|&index| {
                self.history.transcript.get(index).is_some_and(|entry| {
                    expandable_tool_entry(entry, self.info.runtime.max_tool_output_lines)
                })
            }) {
                self.toggle_transcript_tool_output(index);
                self.clamp_history_scroll_for_terminal(terminal)?;
                return Ok(true);
            }
        }

        let static_len = self.history_static_len(width);
        let mut pending_start = static_len;
        let transcript_ends_with_tool = self.history.last_inserted_was_tool
            || self.history.transcript.last().is_some_and(is_tool_entry);
        for (shell_index, shell) in self.running_inline_shell_entries().enumerate() {
            if shell_index > 0 || transcript_ends_with_tool {
                pending_start = pending_start.saturating_add(1);
            }
            pending_start = pending_start.saturating_add(
                tool_entry_lines(&shell, width, self.info.runtime.max_tool_output_lines).len(),
            );
        }
        enum PendingToolKey {
            Preview(usize),
            Running(rho_sdk::ToolCallId),
        }
        let mut target = None;
        let entries = self
            .tool_calls
            .previews
            .iter()
            .map(|(index, entry)| (PendingToolKey::Preview(*index), entry))
            .chain(
                self.tool_calls
                    .running
                    .iter()
                    .map(|(call_id, entry)| (PendingToolKey::Running(call_id.clone()), entry)),
            );
        for (key, pending) in entries {
            if transcript_ends_with_tool {
                pending_start = pending_start.saturating_add(1);
            }
            let pending_end = pending_start.saturating_add(
                tool_entry_lines(pending, width, self.info.runtime.max_tool_output_lines).len(),
            );
            if (pending_start..pending_end).contains(&line)
                && tool_display_line_count(&pending.display_lines)
                    > self.info.runtime.max_tool_output_lines
            {
                target = Some(key);
                break;
            }
            pending_start = pending_end;
        }
        if let Some(target) = target {
            let pending = match target {
                PendingToolKey::Preview(index) => self.tool_calls.previews.get_mut(&index),
                PendingToolKey::Running(call_id) => self.tool_calls.running.get_mut(&call_id),
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
        self.history.copy_notice = Some(CopyNotice::from_copy_result(
            self.clipboard.copy(text),
            character_count,
            now,
        ));
    }
}
