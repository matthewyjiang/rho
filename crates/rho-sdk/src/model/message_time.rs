//! When a conversation entry was created, as the model reads it.
//!
//! The time is captured once, stored with the entry, and rendered from the
//! stored value on every request, so replays and resumed sessions send the
//! same bytes and keep the provider prompt cache warm.

use std::fmt;

use chrono::{DateTime, FixedOffset, Local, Offset, Utc};
use serde::{Deserialize, Serialize};

use super::{ContentBlock, Message};

/// A wall-clock instant and the UTC offset of the zone it was read in.
///
/// Renders as one line, such as `Thursday, Nov 12, 2026, 12:39 AM (UTC-5)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MessageTimeRepr", into = "MessageTimeRepr")]
pub struct MessageTime {
    at: DateTime<FixedOffset>,
}

impl MessageTime {
    /// The current time in the machine's local zone.
    pub fn now() -> Self {
        let now = Local::now();
        Self {
            at: now.with_timezone(&now.offset().fix()),
        }
    }

    /// `unix_seconds` read in a zone `utc_offset_seconds` east of UTC; `None`
    /// when either is out of range.
    pub fn from_parts(unix_seconds: i64, utc_offset_seconds: i32) -> Option<Self> {
        let offset = FixedOffset::east_opt(utc_offset_seconds)?;
        let at = DateTime::<Utc>::from_timestamp(unix_seconds, 0)?;
        Some(Self {
            at: at.with_timezone(&offset),
        })
    }

    /// Puts this time on its own line before the first text block, so block
    /// positions recorded against `content` stay valid. Returns `false`, and
    /// leaves `content` unchanged, when it has no text block.
    pub(crate) fn prefix_first_text(&self, content: &mut [ContentBlock]) -> bool {
        let Some(text) = content.iter_mut().find_map(|block| match block {
            ContentBlock::Text(text) => Some(text),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        }) else {
            return false;
        };
        *text = format!("{self}\n{text}");
        true
    }
}

/// Moves an assistant entry's [`super::AssistantMessage::created_at`] into its
/// content, as provider requests send it. Other entries carry their stamp in
/// their content already, or have none.
pub(crate) fn render_for_request(message: &mut Message) {
    if let Message::EnrichedAssistant(assistant) = message {
        if let Some(time) = assistant.created_at.take() {
            time.prefix_first_text(&mut assistant.content);
        }
    }
}

/// Whether [`render_for_request`] would change `message`.
pub(crate) fn renders_in_request(message: &Message) -> bool {
    matches!(message, Message::EnrichedAssistant(assistant) if assistant.created_at.is_some())
}

impl fmt::Display for MessageTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (UTC", self.at.format("%A, %b %-d, %Y, %-I:%M %p"))?;
        let offset = self.at.offset().local_minus_utc();
        if offset != 0 {
            let sign = if offset < 0 { '-' } else { '+' };
            let minutes = offset.unsigned_abs() / 60;
            let (hours, minutes) = (minutes / 60, minutes % 60);
            write!(f, "{sign}{hours}")?;
            if minutes != 0 {
                write!(f, ":{minutes:02}")?;
            }
        }
        f.write_str(")")
    }
}

#[derive(Serialize, Deserialize)]
struct MessageTimeRepr {
    unix_seconds: i64,
    utc_offset_seconds: i32,
}

impl TryFrom<MessageTimeRepr> for MessageTime {
    type Error = String;

    fn try_from(repr: MessageTimeRepr) -> Result<Self, Self::Error> {
        Self::from_parts(repr.unix_seconds, repr.utc_offset_seconds).ok_or_else(|| {
            format!(
                "message time out of range: {} seconds at offset {}",
                repr.unix_seconds, repr.utc_offset_seconds
            )
        })
    }
}

impl From<MessageTime> for MessageTimeRepr {
    fn from(time: MessageTime) -> Self {
        Self {
            unix_seconds: time.at.timestamp(),
            utc_offset_seconds: time.at.offset().local_minus_utc(),
        }
    }
}

#[cfg(test)]
#[path = "message_time_tests.rs"]
mod tests;
