//! `formatDate`.

use std::fmt::Write as _;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike};
use serde_json::Value;

use crate::functions::Args;

/// The pattern tokens, longest first wherever one is a prefix of another.
const TOKENS: [&str; 17] = [
    "yyyy", "yy", "MMMM", "MMM", "MM", "M", "EEEE", "E", "dd", "d", "HH", "H", "hh", "h", "mm",
    "ss", "a",
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// `formatDate(value, format)`: an ISO 8601 timestamp written with a TR35
/// pattern, in the wall-clock time it was written with. A timestamp without
/// an offset is read as written, never in the host's time zone. The tokens
/// are `yyyy yy MMMM MMM MM M EEEE E dd d HH H hh h mm ss a`, with English
/// names; other text is copied. The pattern `ISO` writes the UTC instant as
/// `YYYY-MM-DDTHH:mm:ss.sssZ`. A value that is not a valid timestamp, such
/// as `2026-02-30`, formats as an empty string.
pub(crate) fn format_date(args: &Args) -> Value {
    let Some(timestamp) = Timestamp::parse(&args.string("value")) else {
        return Value::String(String::new());
    };
    let pattern = args.string("format");
    Value::String(match pattern.as_str() {
        "ISO" => timestamp.iso(),
        "" => timestamp.format("yyyy-MM-dd"),
        pattern => timestamp.format(pattern),
    })
}

/// A parsed timestamp: its fields as written, and its offset from UTC.
struct Timestamp {
    wall: NaiveDateTime,
    offset_minutes: i64,
}

impl Timestamp {
    /// Reads `YYYY-MM-DD` (dashes optional), optionally followed by `T` or a
    /// space and `HH`, `HH:mm`, `HH:mm:ss` or `HH:mm:ss.fff` (colons
    /// optional, `,` allowed before the fraction), and an offset `Z`, `±HH`,
    /// `±HHmm` or `±HH:mm`. Fields must name a real date and time.
    fn parse(text: &str) -> Option<Self> {
        let mut cursor = Cursor(text);
        let year = cursor.number(4)?;
        cursor.eat('-');
        let month = cursor.number(2)?;
        cursor.eat('-');
        let day = cursor.number(2)?;
        let date = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?;
        let mut time = [0; 3];
        let mut millis = 0;
        let mut offset_minutes = 0;
        if cursor.eat('T') || cursor.eat(' ') {
            for (i, field) in time.iter_mut().enumerate() {
                if i > 0 {
                    cursor.eat(':');
                    if !cursor.0.starts_with(|c: char| c.is_ascii_digit()) {
                        break;
                    }
                }
                *field = cursor.number(2)?;
            }
            if cursor.eat('.') || cursor.eat(',') {
                let digits = cursor.digits();
                if digits.is_empty() {
                    return None;
                }
                millis = format!("{:0<3}", &digits[..digits.len().min(3)])
                    .parse()
                    .ok()?;
            }
            offset_minutes = cursor.offset()?;
        }
        if !cursor.0.is_empty() {
            return None;
        }
        let time = NaiveTime::from_hms_milli_opt(time[0], time[1], time[2], millis)?;
        Some(Self {
            wall: date.and_time(time),
            offset_minutes,
        })
    }

    /// The UTC instant, as JavaScript's `Date.prototype.toISOString` writes it.
    fn iso(&self) -> String {
        let instant = self.wall - TimeDelta::minutes(self.offset_minutes);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            instant.year(),
            instant.month(),
            instant.day(),
            instant.hour(),
            instant.minute(),
            instant.second(),
            instant.nanosecond() / 1_000_000,
        )
    }

    fn format(&self, pattern: &str) -> String {
        let wall = &self.wall;
        let hour12 = match wall.hour() % 12 {
            0 => 12,
            hour => hour,
        };
        let month = MONTHS[usize::try_from(wall.month0()).unwrap_or(0)];
        let weekday = WEEKDAYS[usize::try_from(wall.weekday().num_days_from_sunday()).unwrap_or(0)];
        let mut out = String::new();
        let mut rest = pattern;
        while let Some(c) = rest.chars().next() {
            let Some(token) = TOKENS.into_iter().find(|token| rest.starts_with(token)) else {
                out.push(c);
                rest = &rest[c.len_utf8()..];
                continue;
            };
            let _ = match token {
                "yyyy" => write!(out, "{}", wall.year()),
                "yy" => write!(out, "{:02}", wall.year().rem_euclid(100)),
                "MMMM" => write!(out, "{month}"),
                "MMM" => write!(out, "{}", &month[..3]),
                "MM" => write!(out, "{:02}", wall.month()),
                "M" => write!(out, "{}", wall.month()),
                "EEEE" => write!(out, "{weekday}"),
                "E" => write!(out, "{}", &weekday[..3]),
                "dd" => write!(out, "{:02}", wall.day()),
                "d" => write!(out, "{}", wall.day()),
                "HH" => write!(out, "{:02}", wall.hour()),
                "H" => write!(out, "{}", wall.hour()),
                "hh" => write!(out, "{hour12:02}"),
                "h" => write!(out, "{hour12}"),
                "mm" => write!(out, "{:02}", wall.minute()),
                "ss" => write!(out, "{:02}", wall.second()),
                _ => write!(out, "{}", if wall.hour() < 12 { "AM" } else { "PM" }),
            };
            rest = &rest[token.len()..];
        }
        out
    }
}

struct Cursor<'s>(&'s str);

impl<'s> Cursor<'s> {
    fn eat(&mut self, c: char) -> bool {
        match self.0.strip_prefix(c) {
            Some(rest) => {
                self.0 = rest;
                true
            }
            None => false,
        }
    }

    fn digits(&mut self) -> &'s str {
        let end = self
            .0
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(self.0.len());
        let (digits, rest) = self.0.split_at(end);
        self.0 = rest;
        digits
    }

    /// Exactly `len` ASCII digits.
    fn number(&mut self, len: usize) -> Option<u32> {
        let digits = self.0.get(..len)?;
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        self.0 = &self.0[len..];
        digits.parse().ok()
    }

    /// An optional offset from UTC, in minutes.
    fn offset(&mut self) -> Option<i64> {
        if self.eat('Z') || self.eat('z') {
            return Some(0);
        }
        let sign = if self.eat('+') {
            1
        } else if self.eat('-') {
            -1
        } else {
            return Some(0);
        };
        let hours = self.number(2)?;
        self.eat(':');
        let minutes = if self.0.is_empty() {
            0
        } else {
            self.number(2)?
        };
        Some(sign * i64::from(hours * 60 + minutes))
    }
}
