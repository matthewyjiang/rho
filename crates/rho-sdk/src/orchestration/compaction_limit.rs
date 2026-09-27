//! How far automatic compaction may reach while async tool jobs run.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{ContentBlock, Message, SemanticMessage};

/// Where running async tool calls stop automatic compaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PendingAsyncCalls {
    /// No async job is running; the whole history is compactable.
    None,
    /// Compact only `history[..end]`. The rest, including every running call,
    /// stays verbatim so late results still pair with their calls.
    CompactBefore(usize),
    /// Running calls leave nothing worth compacting before them.
    Blocking,
}

impl PendingAsyncCalls {
    /// Locates the compactable prefix ahead of the `running` provider call IDs.
    ///
    /// The prefix ends at the earliest assistant message holding a running
    /// call, then moves earlier until no tool result or tool image after it is
    /// separated from its call. A prefix without a model reply holds only
    /// instructions and user input, so it blocks instead. A running call
    /// missing from history also blocks, since no safe cut is known.
    pub(super) fn locate(history: &[Message], running: &BTreeSet<&str>) -> Self {
        if running.is_empty() {
            return Self::None;
        }
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
        let Some(mut end) = running
            .iter()
            .map(|id| call_owners.get(id).copied())
            .collect::<Option<Vec<_>>>()
            .and_then(|owners| owners.into_iter().min())
        else {
            return Self::Blocking;
        };
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
        if history[..end]
            .iter()
            .any(|message| message.completed_assistant_content().is_some())
        {
            Self::CompactBefore(end)
        } else {
            Self::Blocking
        }
    }
}

#[cfg(test)]
#[path = "compaction_limit_tests.rs"]
mod tests;
