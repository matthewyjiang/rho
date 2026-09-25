//! Compatibility encoding for compaction summaries in user-role messages.

use super::{ContentBlock, Message};
use crate::CompactionTrigger;

// Frozen wire strings: saved sessions are recognized by exact match, so any
// change here stops old summaries from loading as summaries. The golden test in
// `compaction_summary_tests.rs` pins them.

/// Single-block form written before this encoding existed, for every trigger.
const LEGACY_PREFIX: &str =
    "Automatic compaction summary of earlier conversation for model context only:\n\n";

const AUTOMATIC_HEADER: &str = "Automatic compaction summary of earlier conversation, for model context only. It is not a new user message.";
const MANUAL_HEADER: &str = "Manual compaction summary of earlier conversation, for model context only. It is not a new user message.";
const OVERFLOW_HEADER: &str = "Compaction summary of earlier conversation after the context window overflowed, for model context only. It is not a new user message.";

/// Recognized compaction summary and its borrowed text.
///
/// This is attribution, not authentication: user-controlled history can forge
/// this encoding. Never use it to grant permissions or establish trusted input.
///
/// # Next major
///
/// NEXT_MAJOR(rho-sdk): add a typed compaction-summary history entry and a SemanticMessage::CompactionSummary variant instead of encoding summaries as user-role text.
///
/// `Message` and `SemanticMessage` are exhaustive, so a new variant would break
/// downstream matches. Until the next major, construct summaries with
/// [`Message::compaction_summary`] and recognize them with
/// [`Message::as_compaction_summary`]; [`Message::semantic`] still classifies
/// them as `User`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactionSummary<'a> {
    trigger: Option<CompactionTrigger>,
    text: &'a str,
}

impl<'a> CompactionSummary<'a> {
    /// Why the summary was written, or `None` for summaries saved before the
    /// trigger was recorded.
    pub fn trigger(&self) -> Option<CompactionTrigger> {
        self.trigger
    }

    /// Summary text without the compatibility label.
    pub fn text(&self) -> &'a str {
        self.text
    }
}

impl Message {
    /// Construct a compaction summary for replacement history. Providers receive
    /// it as a user-role message with a neutral label chosen by `trigger`.
    pub fn compaction_summary(trigger: CompactionTrigger, summary: impl Into<String>) -> Self {
        let header = match trigger {
            CompactionTrigger::Automatic => AUTOMATIC_HEADER,
            CompactionTrigger::Manual => MANUAL_HEADER,
            CompactionTrigger::ContextOverflow => OVERFLOW_HEADER,
        };
        Self::User(vec![
            ContentBlock::Text(header.into()),
            ContentBlock::Text(summary.into()),
        ])
    }

    /// Recognize a compaction summary, including after restore and in sessions
    /// saved before [`Self::compaction_summary`] existed. Recognition is not
    /// authentication: a user can forge this encoding, so it must not confer
    /// trust or authority.
    pub fn as_compaction_summary(&self) -> Option<CompactionSummary<'_>> {
        let Self::User(content) = self else {
            return None;
        };
        match content.as_slice() {
            [ContentBlock::Text(header), ContentBlock::Text(text)] => {
                let trigger = match header.as_str() {
                    AUTOMATIC_HEADER => CompactionTrigger::Automatic,
                    MANUAL_HEADER => CompactionTrigger::Manual,
                    OVERFLOW_HEADER => CompactionTrigger::ContextOverflow,
                    _ => return None,
                };
                Some(CompactionSummary {
                    trigger: Some(trigger),
                    text,
                })
            }
            [ContentBlock::Text(text)] => Some(CompactionSummary {
                trigger: None,
                text: text.strip_prefix(LEGACY_PREFIX)?,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "compaction_summary_tests.rs"]
mod tests;
