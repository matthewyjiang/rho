//! Hit-test and accordion toggle targets for attach tool cards.
//!
//! [`PaintedHistory`] paints the visible items once into a line stack plus
//! per-card spans, so rendering, hover lift, and click mapping share one line
//! index. The attach app caches it between content and layout changes instead
//! of re-rendering the whole transcript on every event.

use std::ops::Range;

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
            items.push(HistoryItem::Ephemeral(Entry::Assistant(
                text.to_string().into(),
            )));
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

/// One painted history render: the full line stack plus tool-card spans.
///
/// Lines and spans come from the same paint pass, so hit-testing always
/// agrees with what is on screen.
pub(super) struct PaintedHistory {
    /// Width the lines were wrapped for; a mismatch invalidates the cache.
    pub(super) width: usize,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) cards: Vec<PaintedCard>,
}

/// Paint-order metadata for one tool card inside [`PaintedHistory`].
pub(super) struct PaintedCard {
    /// Set only when the card is over budget and has a click/Ctrl+O target.
    toggle: Option<ToggleTarget>,
    span: Range<usize>,
    pending: bool,
}

impl PaintedHistory {
    pub(super) fn paint<'a, I>(items: I, width: usize, max_tool_output_lines: usize) -> Self
    where
        I: IntoIterator<Item = HistoryItem<'a>>,
    {
        let mut lines = Vec::new();
        let mut cards = Vec::new();
        for item in items {
            let painted = item.paint_lines(width, max_tool_output_lines);
            if painted.is_empty() {
                continue;
            }
            let span = lines.len()..lines.len().saturating_add(painted.len());
            if item.tool_entry().is_some() {
                cards.push(PaintedCard {
                    toggle: item
                        .is_toggleable(width, max_tool_output_lines)
                        .then(|| item.toggle_target())
                        .flatten(),
                    span,
                    pending: matches!(item, HistoryItem::Pending { .. }),
                });
            }
            lines.extend(painted);
        }
        Self {
            width,
            lines,
            cards,
        }
    }
}

/// Toggleable tool card covering `line`: its target and full line span.
///
/// The span is the whole clickable card, so hover lift and click toggle
/// agree on the hit region.
pub(super) fn tool_card_at_line(
    cards: &[PaintedCard],
    line: usize,
) -> Option<(ToggleTarget, Range<usize>)> {
    let card = cards.iter().find(|card| card.span.contains(&line))?;
    Some((card.toggle.clone()?, card.span.clone()))
}

/// Latest Ctrl+O target in paint order.
///
/// Pending cards paint after transcript. While any pending card exists, only
/// the latest pending card is eligible — matching the main TUI, which does
/// not fall back to an older pending or transcript card when that latest
/// pending body is under budget.
pub(super) fn latest_toggle_target(cards: &[PaintedCard]) -> Option<ToggleTarget> {
    let candidate = cards
        .iter()
        .rfind(|card| card.pending)
        .or_else(|| cards.iter().rfind(|card| card.toggle.is_some()))?;
    candidate.toggle.clone()
}

#[cfg(test)]
#[path = "tool_toggle_tests.rs"]
mod tests;
