//! Expand and collapse truncated tool output in live batches and transcript history.

use ratatui::DefaultTerminal;

use super::{App, Entry};

pub(super) fn expandable_tool_entry(entry: &Entry, max_tool_output_lines: usize) -> bool {
    matches!(
        entry,
        Entry::Tool(tool) if tool_expandable_line_count(tool) > max_tool_output_lines
    )
}

pub(super) fn tool_expandable_line_count(tool: &super::ToolEntry) -> usize {
    // Diff bodies are hidden until expanded; other bodies use the line budget.
    if matches!(tool.card.body, rho_tools::tool_card::ToolBody::DiffLines(_)) {
        return tool.card.body.line_count().max(1);
    }
    tool.card.expandable_line_count()
}

impl App {
    pub(super) fn toggle_latest_tool_output(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        if let Some(pending) = self.turn.latest_tool_mut() {
            if tool_expandable_line_count(pending) <= self.info.runtime.max_tool_output_lines {
                self.status = "no truncated tool output".into();
                return Ok(());
            }
            pending.expanded = !pending.expanded;
            self.status = if pending.expanded {
                "tool output expanded".into()
            } else {
                "tool output collapsed".into()
            };
            return Ok(());
        }

        let Some(index) = self.history.entries().iter().rposition(|entry| {
            expandable_tool_entry(entry, self.info.runtime.max_tool_output_lines)
        }) else {
            self.status = "no truncated tool output".into();
            return Ok(());
        };

        self.toggle_transcript_tool_output(index);
        self.clamp_history_scroll_for_terminal(terminal)
    }

    pub(super) fn toggle_transcript_tool_output(&mut self, index: usize) {
        let expand = !matches!(self.history.get(index), Some(Entry::Tool(tool)) if tool.expanded);
        let mut dirty_from = index;
        for (entry_index, entry) in self.history.entries_mut().iter_mut().enumerate() {
            if let Entry::Tool(tool) = entry {
                if tool.expanded {
                    dirty_from = dirty_from.min(entry_index);
                }
                tool.expanded = false;
            }
        }
        if let Some(Entry::Tool(tool)) = self.history.get_mut(index) {
            tool.expanded = expand;
            self.history.lines_mut().invalidate_from(dirty_from);
        }
        self.status = if expand {
            "tool output expanded".into()
        } else {
            "tool output collapsed".into()
        };
    }
}
