//! Expand and collapse truncated tool output in live batches and transcript history.

use ratatui::DefaultTerminal;

use super::{tool_card_render::card_is_toggleable, App, Entry};

/// Default history width used when a toggle check has no live terminal size yet.
const TOGGLE_WIDTH_FALLBACK: usize = 80;

pub(super) fn expandable_tool_entry(
    entry: &Entry,
    max_tool_output_lines: usize,
    width: usize,
) -> bool {
    matches!(
        entry,
        Entry::Tool(tool) if tool_output_toggleable(tool, max_tool_output_lines, width)
    )
}

/// Whether ctrl+o / click should toggle this tool's expanded body.
pub(super) fn tool_output_toggleable(
    tool: &super::ToolEntry,
    max_tool_output_lines: usize,
    width: usize,
) -> bool {
    let width = width.max(1);
    card_is_toggleable(&tool.card, width, max_tool_output_lines, tool.expanded)
}

impl App {
    pub(super) fn toggle_latest_tool_output(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        let width = terminal
            .size()
            .map(|size| size.width as usize)
            .unwrap_or(TOGGLE_WIDTH_FALLBACK);
        let status = if let Some(pending) = self.turn.latest_tool_mut() {
            if !tool_output_toggleable(pending, self.info.runtime.max_tool_output_lines, width) {
                Some("no truncated tool output")
            } else {
                pending.expanded = !pending.expanded;
                Some(if pending.expanded {
                    "tool output expanded"
                } else {
                    "tool output collapsed"
                })
            }
        } else {
            None
        };
        if let Some(status) = status {
            self.set_status(status);
            return Ok(());
        }

        let Some(index) = self.history.entries().iter().rposition(|entry| {
            expandable_tool_entry(entry, self.info.runtime.max_tool_output_lines, width)
        }) else {
            self.set_status("no truncated tool output");
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
        self.set_status(if expand {
            "tool output expanded"
        } else {
            "tool output collapsed"
        });
    }
}
