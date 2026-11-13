//! When user messages were sent, as the model reads it.
//!
//! Only user messages carry a stamp: the time they were sent, written into
//! their content when they are created, so replays and resumed sessions send
//! the same bytes and keep the provider prompt cache warm. Assistant output
//! stays exactly as the model wrote it.

use std::fmt;

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, Offset, TimeZone, Utc};
use serde::{Deserialize, Serialize};

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

    /// The rest of `text` after a leading line that is exactly a rendered
    /// time, or `None` when it has none. For readers that must see past the
    /// stamp to the words the user typed, such as scripted test providers.
    pub fn strip_rendered_prefix(text: &str) -> Option<&str> {
        let (line, rest) = text.split_once('\n').unwrap_or((text, ""));
        let time = Self::parse(line)?;
        (time.to_string() == line).then_some(rest)
    }

    /// Reads a time [`fmt::Display`] wrote; the caller checks it round-trips.
    fn parse(line: &str) -> Option<Self> {
        let (local, zone) = line.strip_suffix(')')?.rsplit_once(" (UTC")?;
        let offset_seconds = match zone {
            "" => 0,
            zone => {
                let (sign, magnitude) = match zone.as_bytes().first()? {
                    b'+' => (1, &zone[1..]),
                    b'-' => (-1, &zone[1..]),
                    _ => return None,
                };
                let (hours, minutes) = magnitude.split_once(':').unwrap_or((magnitude, "0"));
                sign * (hours.parse::<i32>().ok()? * 3600 + minutes.parse::<i32>().ok()? * 60)
            }
        };
        let offset = FixedOffset::east_opt(offset_seconds)?;
        let local = NaiveDateTime::parse_from_str(local, "%A, %b %d, %Y, %I:%M %p").ok()?;
        Some(Self {
            at: offset.from_local_datetime(&local).single()?,
        })
    }
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
