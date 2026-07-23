//! Expand and collapse truncated tool output in live batches and transcript history.

use ratatui::DefaultTerminal;

use super::{expandable_tool_entry, tool_display_line_count, App, Entry};

impl App {
    pub(super) fn toggle_latest_tool_output(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        if let Some(pending) = self.tool_calls.latest_mut() {
            if tool_display_line_count(&pending.display_lines)
                <= self.info.runtime.max_tool_output_lines
            {
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

        let Some(index) = self.transcript.iter().rposition(|entry| {
            expandable_tool_entry(entry, self.info.runtime.max_tool_output_lines)
        }) else {
            self.status = "no truncated tool output".into();
            return Ok(());
        };

        self.toggle_transcript_tool_output(index);
        self.clamp_history_scroll_for_terminal(terminal)
    }

    pub(super) fn toggle_transcript_tool_output(&mut self, index: usize) {
        let expand =
            !matches!(self.transcript.get(index), Some(Entry::Tool(tool)) if tool.expanded);
        let mut dirty_from = index;
        for (entry_index, entry) in self.transcript.iter_mut().enumerate() {
            if let Entry::Tool(tool) = entry {
                if tool.expanded {
                    dirty_from = dirty_from.min(entry_index);
                }
                tool.expanded = false;
            }
        }
        if let Some(Entry::Tool(tool)) = self.transcript.get_mut(index) {
            tool.expanded = expand;
            self.history_lines.invalidate_from(dirty_from);
        }
        self.status = if expand {
            "tool output expanded".into()
        } else {
            "tool output collapsed".into()
        };
    }
}
