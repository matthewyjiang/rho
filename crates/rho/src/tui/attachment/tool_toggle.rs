//! Hit-test and accordion toggle targets for attach tool cards.
//!
//! Attach rebuilds history on every draw, so click mapping walks the same
//! visible items `history_lines` paints instead of a cached line index.

use ratatui::text::Line;

use crate::subagent::RunStatus;

use super::super::{
    feed_image::DEFAULT_IMAGE_HEIGHT,
    render::{entry_lines, tool_entry_lines},
    tool_output_ui::tool_output_toggleable,
    Entry, ToolEntry,
};

/// A tool card the user can expand or collapse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToggleTarget {
    Transcript(usize),
    Pending(String),
}

/// One painted history block, in the same order as attach draw.
pub(super) enum HistoryItem<'a> {
    Transcript { index: usize, entry: &'a Entry },
    Pending { key: &'a str, tool: &'a ToolEntry },
    Ephemeral(Entry),
}

impl HistoryItem<'_> {
    pub(super) fn paint_lines(
        &self,
        width: usize,
        max_tool_output_lines: usize,
    ) -> Vec<Line<'static>> {
        match self {
            Self::Transcript { entry, .. } => {
                entry_lines(entry, width, max_tool_output_lines, DEFAULT_IMAGE_HEIGHT)
            }
            Self::Ephemeral(entry) => {
                entry_lines(entry, width, max_tool_output_lines, DEFAULT_IMAGE_HEIGHT)
            }
            Self::Pending { tool, .. } => {
                tool_entry_lines(tool, width, max_tool_output_lines, DEFAULT_IMAGE_HEIGHT)
            }
        }
    }

    fn tool_entry(&self) -> Option<&ToolEntry> {
        match self {
            Self::Transcript {
                entry: Entry::Tool(tool),
                ..
            }
            | Self::Ephemeral(Entry::Tool(tool)) => Some(tool),
            Self::Pending { tool, .. } => Some(tool),
            _ => None,
        }
    }

    fn toggle_target(&self) -> Option<ToggleTarget> {
        match self {
            Self::Transcript {
                index,
                entry: Entry::Tool(_),
            } => Some(ToggleTarget::Transcript(*index)),
            Self::Pending { key, .. } => Some(ToggleTarget::Pending((*key).to_string())),
            _ => None,
        }
    }

    fn is_toggleable(&self, width: usize, max_tool_output_lines: usize) -> bool {
        self.tool_entry()
            .is_some_and(|tool| tool_output_toggleable(tool, max_tool_output_lines, width))
    }
}

/// Status-only assistant/error rows that paint after the journal.
pub(super) fn status_fallback_items(
    status: Option<&RunStatus>,
    has_assistant: bool,
) -> Vec<HistoryItem<'static>> {
    let mut items = Vec::new();
    if !has_assistant {
        let fallback = status.and_then(|status| {
            status
                .result
                .as_deref()
                .or(status.last_text.as_deref())
                .filter(|text| !text.is_empty())
        });
        if let Some(text) = fallback {
            items.push(HistoryItem::Ephemeral(Entry::Assistant(text.to_string())));
        }
    }
    if let Some(error) = status.and_then(|status| status.error.as_deref()) {
        items.push(HistoryItem::Ephemeral(Entry::Error(error.to_string())));
    }
    if let Some(error) = status.and_then(|status| status.attachment_error.as_deref()) {
        items.push(HistoryItem::Ephemeral(Entry::Error(error.to_string())));
    }
    items
}

/// Toggleable tool card covering `line`: its target and full line span.
///
/// The span is the whole clickable card, so hover lift and click toggle
/// agree on the hit region.
pub(super) fn tool_card_at_line<'a, I>(
    items: I,
    line: usize,
    width: usize,
    max_tool_output_lines: usize,
) -> Option<(ToggleTarget, std::ops::Range<usize>)>
where
    I: IntoIterator<Item = HistoryItem<'a>>,
{
    let mut start = 0usize;
    for item in items {
        let height = item.paint_lines(width, max_tool_output_lines).len();
        if height == 0 {
            continue;
        }
        let end = start.saturating_add(height);
        if (start..end).contains(&line) {
            return item
                .is_toggleable(width, max_tool_output_lines)
                .then(|| item.toggle_target())
                .flatten()
                .map(|target| (target, start..end));
        }
        start = end;
    }
    None
}

/// Map a visible history line onto a toggleable tool, if any.
pub(super) fn tool_target_at_line<'a, I>(
    items: I,
    line: usize,
    width: usize,
    max_tool_output_lines: usize,
) -> Option<ToggleTarget>
where
    I: IntoIterator<Item = HistoryItem<'a>>,
{
    tool_card_at_line(items, line, width, max_tool_output_lines).map(|(target, _)| target)
}

/// Latest Ctrl+O target in paint order.
///
/// Pending cards paint after transcript. While any pending card exists, only
/// the latest pending card is eligible — matching the main TUI, which does
/// not fall back to an older pending or transcript card when that latest
/// pending body is under budget.
pub(super) fn latest_toggle_target<'a, I>(
    items: I,
    width: usize,
    max_tool_output_lines: usize,
) -> Option<ToggleTarget>
where
    I: IntoIterator<Item = HistoryItem<'a>>,
{
    let mut last_pending = None;
    let mut last_toggleable = None;
    for item in items {
        if matches!(item, HistoryItem::Pending { .. }) {
            last_pending = Some(item);
        } else if item.is_toggleable(width, max_tool_output_lines) {
            last_toggleable = Some(item);
        }
    }
    let candidate = match last_pending {
        Some(pending) => pending,
        None => last_toggleable?,
    };
    candidate
        .is_toggleable(width, max_tool_output_lines)
        .then(|| candidate.toggle_target())
        .flatten()
}

#[cfg(test)]
#[path = "tool_toggle_tests.rs"]
mod tests;
