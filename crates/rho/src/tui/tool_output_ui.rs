//! Expand and collapse truncated tool output in live batches and transcript history.

use ratatui::DefaultTerminal;

use super::{App, Entry};

pub(super) fn expandable_tool_entry(entry: &Entry, max_tool_output_lines: usize) -> bool {
    matches!(
        entry,
        Entry::Tool(tool) if tool_output_toggleable(tool, max_tool_output_lines)
    )
}

/// Whether ctrl+o / click should toggle this tool's expanded body.
pub(super) fn tool_output_toggleable(
    tool: &super::ToolEntry,
    max_tool_output_lines: usize,
) -> bool {
    let collapsed = tool
        .card
        .display_plan(max_tool_output_lines, /*expanded*/ false);
    if tool.expanded {
        let expanded_plan = tool
            .card
            .display_plan(max_tool_output_lines, /*expanded*/ true);
        expanded_plan.show_collapse_prompt || collapsed.expandable
    } else {
        collapsed.expandable
    }
}

impl App {
    pub(super) fn toggle_latest_tool_output(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        if let Some(pending) = self.turn.latest_tool_mut() {
            if !tool_output_toggleable(pending, self.info.runtime.max_tool_output_lines) {
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
