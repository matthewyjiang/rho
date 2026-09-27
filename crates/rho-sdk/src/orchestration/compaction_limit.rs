//! How much history one step's automatic or overflow compaction may rewrite.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    model::{ContentBlock, Message, SemanticMessage},
    CompactionExtent,
};

/// The history prefix compaction may rewrite during one model step.
///
/// Everything after the limit reaches the next request verbatim. Messages
/// only ever append after it, so a limit stays valid for the whole step once
/// re-addressed across a compaction with [`Self::shifted`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompactionLimit {
    /// No async job runs. Compact all history before `preserve_from`, the
    /// fresh completion input, or all of it.
    History { preserve_from: Option<usize> },
    /// Compact only `history[..end]`. The rest holds every running async call,
    /// so their late results still pair with their calls.
    BeforePendingCalls(usize),
    /// Running async calls leave nothing new to compact before them.
    BlockedByPendingCalls,
}

impl CompactionLimit {
    /// Limit for a step whose `running` provider call IDs have no result yet.
    ///
    /// With running calls, the prefix ends at the earliest assistant message
    /// holding one, then moves earlier until no tool result or tool image
    /// after it is separated from its call, and before any `preserve_from`.
    /// The limit blocks when:
    /// - the prefix holds no model reply, only instructions and user input;
    /// - a running call is missing from history, so no safe cut is known;
    /// - the prefix ends at or before `compacted_end`, the end of the prefix
    ///   an earlier step already compacted while the same jobs ran. Nothing
    ///   new reaches that prefix until the earliest running call finishes.
    pub(super) fn for_step(
        history: &[Message],
        running: &BTreeSet<&str>,
        preserve_from: Option<usize>,
        compacted_end: Option<usize>,
    ) -> Self {
        if running.is_empty() {
            return Self::History { preserve_from };
        }
        let end = pending_prefix_end(history, running)
            .map(|end| preserve_from.map_or(end, |start| start.min(end)));
        match end {
            Some(end) if compacted_end.is_none_or(|compacted| end > compacted) => {
                Self::BeforePendingCalls(end)
            }
            Some(_) | None => Self::BlockedByPendingCalls,
        }
    }

    /// End of the compactable prefix of `history`, or `None` when blocked.
    pub(super) fn compact_end(self, history_len: usize) -> Option<usize> {
        match self {
            Self::History { preserve_from } => Some(preserve_from.unwrap_or(history_len)),
            Self::BeforePendingCalls(end) => Some(end),
            Self::BlockedByPendingCalls => None,
        }
    }

    pub(super) fn extent(self) -> CompactionExtent {
        match self {
            Self::History { .. } => CompactionExtent::History,
            Self::BeforePendingCalls(_) | Self::BlockedByPendingCalls => {
                CompactionExtent::BeforePendingAsyncTools
            }
        }
    }

    /// Re-addresses this limit after compaction replaced the prefix, changing
    /// history length from `old_len` to `new_len`. The protected suffix keeps
    /// its length.
    pub(super) fn shifted(self, old_len: usize, new_len: usize) -> Self {
        let shift = |position: usize| new_len - (old_len - position);
        match self {
            Self::History { preserve_from } => Self::History {
                preserve_from: preserve_from.map(shift),
            },
            Self::BeforePendingCalls(end) => Self::BeforePendingCalls(shift(end)),
            Self::BlockedByPendingCalls => Self::BlockedByPendingCalls,
        }
    }
}

/// See [`CompactionLimit::for_step`]; `None` when no safe, useful cut exists.
fn pending_prefix_end(history: &[Message], running: &BTreeSet<&str>) -> Option<usize> {
    let mut call_owners = BTreeMap::new();
    for (index, message) in history.iter().enumerate() {
        for block in message.completed_assistant_content().unwrap_or_default() {
            match block {
                ContentBlock::ToolCall(call) => {
                    call_owners.entry(call.id.as_str()).or_insert(index);
                }
                ContentBlock::Text(_) | ContentBlock::Image(_) => {}
            }
        }
    }
    let mut end = running
        .iter()
        .map(|id| call_owners.get(id).copied())
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min()?;
    loop {
        let earliest_owner = history[end..]
            .iter()
            .filter_map(|message| match message.semantic() {
                SemanticMessage::ToolResult(result) => call_owners.get(result.id.as_str()),
                SemanticMessage::ToolImageSupplement(images) => {
                    call_owners.get(images.tool_call_id())
                }
                SemanticMessage::System(_)
                | SemanticMessage::User(_)
                | SemanticMessage::Assistant(_)
                | SemanticMessage::EnrichedAssistant(_)
                | SemanticMessage::AbortedAssistant(_) => None,
            })
            .copied()
            .min()
            .unwrap_or(end);
        if earliest_owner >= end {
            break;
        }
        end = earliest_owner;
    }
    history[..end]
        .iter()
        .any(|message| message.completed_assistant_content().is_some())
        .then_some(end)
}

#[cfg(test)]
#[path = "compaction_limit_tests.rs"]
mod tests;
