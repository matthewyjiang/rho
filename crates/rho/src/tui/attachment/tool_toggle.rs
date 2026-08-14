//! Hit-test and accordion toggle targets for attach tool cards.
//!
//! Attach rebuilds history on every draw, so click mapping walks the same
//! visible entries `history_lines` paints instead of a cached line index.

use std::borrow::Cow;

use super::super::{
    feed_image::DEFAULT_IMAGE_HEIGHT, render::entry_lines, tool_output_ui::expandable_tool_entry,
    Entry,
};

/// A tool card the user can expand or collapse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToggleTarget {
    Transcript(usize),
    Pending(String),
}

/// One painted history block, in the same order as [`super::app::AttachmentApp`] draw.
pub(super) struct HistoryItem<'a> {
    pub target: Option<ToggleTarget>,
    pub entry: Cow<'a, Entry>,
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
    let mut start = 0usize;
    for item in items {
        let height = entry_height(item.entry.as_ref(), width, max_tool_output_lines);
        if height == 0 {
            continue;
        }
        let end = start.saturating_add(height);
        if (start..end).contains(&line) {
            return item.target.filter(|_| {
                expandable_tool_entry(item.entry.as_ref(), max_tool_output_lines, width)
            });
        }
        start = end;
    }
    None
}

/// Last toggleable tool in paint order: pending cards win over transcript.
pub(super) fn latest_toggle_target<'a, I>(
    items: I,
    width: usize,
    max_tool_output_lines: usize,
) -> Option<ToggleTarget>
where
    I: IntoIterator<Item = HistoryItem<'a>>,
{
    items
        .into_iter()
        .filter(|item| {
            item.target.is_some()
                && expandable_tool_entry(item.entry.as_ref(), max_tool_output_lines, width)
        })
        .last()
        .and_then(|item| item.target)
}

pub(super) fn entry_height(entry: &Entry, width: usize, max_tool_output_lines: usize) -> usize {
    entry_lines(entry, width, max_tool_output_lines, DEFAULT_IMAGE_HEIGHT).len()
}

#[cfg(test)]
#[path = "tool_toggle_tests.rs"]
mod tests;
