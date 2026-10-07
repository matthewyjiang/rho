//! Current task progress, independent of where its tool card is in the transcript.

use std::time::Instant;

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use crate::tools::todo::{TodoList, TodoStatus};

use super::{
    overlay_panel::{
        overlay_panel_body_width, overlay_panel_layout, terminal_area, PanelBody,
        PanelScrollTarget, PanelState,
    },
    panel_text::indented_wrapped_lines,
    render::wrap_text_lines,
    App, ComposerMode, DefaultTerminal, InteractiveRuntime, PanelOverlay, Theme,
};

#[derive(Debug)]
pub(super) struct TodoOverlay {
    list: Option<TodoList>,
    panel: PanelState,
}

impl TodoOverlay {
    /// Return the wrapped rows and the current item's first row for initial focus.
    fn rows(&self, width: usize) -> (Vec<Line<'static>>, Option<usize>) {
        let Some(list) = self.list.as_ref().filter(|list| !list.todos.is_empty()) else {
            return (
                indented_wrapped_lines("No tasks in this session", 1, width, Theme::dim()),
                None,
            );
        };
        let mut lines = indented_wrapped_lines(&list.summary(), 1, width, Theme::dim());
        lines.push(Line::default());
        let mut current_row = None;
        for item in &list.todos {
            let (marker, style) = match item.status {
                TodoStatus::Completed => ("✓", Theme::dim()),
                TodoStatus::InProgress => {
                    current_row = Some(lines.len());
                    ("▸", Theme::accent().add_modifier(Modifier::BOLD))
                }
                TodoStatus::Pending => ("○", Theme::text()),
            };
            // Continuation rows align under the item text, not its status marker.
            let prefix = format!(" {marker} ");
            let indent = super::display_width(&prefix).min(width.saturating_sub(1));
            for (index, mut line) in
                wrap_text_lines(&item.content, width.saturating_sub(indent).max(1), style)
                    .into_iter()
                    .enumerate()
            {
                let lead = if index == 0 {
                    super::render::truncate_one_line(&prefix, indent)
                } else {
                    " ".repeat(indent)
                };
                line.spans.insert(0, Span::styled(lead, style));
                lines.push(line);
            }
        }
        (lines, current_row)
    }
}

impl PanelBody for TodoOverlay {
    fn state(&self) -> &PanelState {
        &self.panel
    }

    fn state_mut(&mut self) -> &mut PanelState {
        &mut self.panel
    }

    fn title(&self) -> &str {
        "Task progress"
    }

    fn footer(&self) -> &str {
        "↑↓ scroll · PgUp/PgDn · Enter/Esc close"
    }

    fn body_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        self.rows(width).0
    }
}

impl App {
    pub(super) fn execute_todo_command(
        &mut self,
        terminal: &DefaultTerminal,
    ) -> anyhow::Result<()> {
        let mut overlay = TodoOverlay {
            list: self.todo_list.clone(),
            panel: PanelState::default(),
        };
        if let Some(area) = terminal_area(terminal) {
            let (rows, current) = overlay.rows(overlay_panel_body_width(area));
            let visible = overlay_panel_layout(area, rows.len()).body_rows;
            if let Some(current) = current.filter(|row| *row >= visible) {
                overlay.panel.scroll.apply(
                    PanelScrollTarget::Absolute(current.saturating_sub(visible / 2)),
                    rows.len(),
                    visible,
                );
            }
        }
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Todo(overlay)));
        self.set_status_quiet("todo");
        Ok(())
    }

    /// Refresh in idle and running loops. Leave the reader's scroll position alone.
    pub(super) fn refresh_todo_list(&mut self, agent: &InteractiveRuntime) -> bool {
        let list = agent.todo_list();
        if self.todo_list == list {
            return false;
        }
        self.todo_list = list;
        if let ComposerMode::Panel(PanelOverlay::Todo(overlay)) = self.input_ui.composer_mut() {
            overlay.list.clone_from(&self.todo_list);
            // Selection coordinates belonged to the previous checklist.
            overlay.panel.pointer = Default::default();
            return true;
        }
        false
    }
}
