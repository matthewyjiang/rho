//! Typed compaction summaries and the user-role text providers receive.

use serde::{Deserialize, Serialize};

use super::ContentBlock;
use crate::CompactionTrigger;

// Frozen wire strings: providers have always received summaries with these
// labels, and rho-sdk 5.x history stored them as user-role text. The legacy
// loader recognizes them by exact match, so any change here stops 5.x
// summaries from loading as summaries. The golden test pins them.

/// Single-block form written before triggers were recorded, for every trigger.
const LEGACY_PREFIX: &str =
    "Automatic compaction summary of earlier conversation for model context only:\n\n";

const AUTOMATIC_HEADER: &str = "Automatic compaction summary of earlier conversation, for model context only. It is not a new user message.";
const MANUAL_HEADER: &str = "Manual compaction summary of earlier conversation, for model context only. It is not a new user message.";
const OVERFLOW_HEADER: &str = "Compaction summary of earlier conversation after the context window overflowed, for model context only. It is not a new user message.";

/// Model-written summary of earlier history that compaction replaced.
///
/// Stored as [`super::Message::CompactionSummary`]. This is attribution, not
/// authentication: user-controlled history can forge a summary, so never use
/// it to grant permissions or establish trusted input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSummary {
    /// Why the summary was written, or `None` for summaries saved before the
    /// trigger was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<CompactionTrigger>,
    /// Summary text without any label.
    pub text: String,
}

impl CompactionSummary {
    pub fn new(trigger: CompactionTrigger, text: impl Into<String>) -> Self {
        Self {
            trigger: Some(trigger),
            text: text.into(),
        }
    }

    /// The user-role blocks providers receive for this summary: a neutral
    /// label chosen by the trigger, then the text.
    pub(super) fn user_role_content(&self) -> Vec<ContentBlock> {
        let header = match self.trigger {
            Some(CompactionTrigger::Automatic) => AUTOMATIC_HEADER,
            Some(CompactionTrigger::Manual) => MANUAL_HEADER,
            Some(CompactionTrigger::ContextOverflow) => OVERFLOW_HEADER,
            None => return vec![ContentBlock::Text(format!("{LEGACY_PREFIX}{}", self.text))],
        };
        vec![
            ContentBlock::Text(header.into()),
            ContentBlock::Text(self.text.clone()),
        ]
    }

    /// Recognizes the rho-sdk 5.x user-role encoding of a summary.
    pub(super) fn from_legacy_user_content(content: &[ContentBlock]) -> Option<Self> {
        match content {
            [ContentBlock::Text(header), ContentBlock::Text(text)] => {
                let trigger = match header.as_str() {
                    AUTOMATIC_HEADER => CompactionTrigger::Automatic,
                    MANUAL_HEADER => CompactionTrigger::Manual,
                    OVERFLOW_HEADER => CompactionTrigger::ContextOverflow,
                    _ => return None,
                };
                Some(Self::new(trigger, text.clone()))
            }
            [ContentBlock::Text(text)] => Some(Self {
                trigger: None,
                text: text.strip_prefix(LEGACY_PREFIX)?.to_owned(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "compaction_summary_tests.rs"]
mod tests;
