//! Parse Claude Code `/usage` screen text into rate-limit windows.
//!
//! Claude owns the credential; this only reads the panel the TUI already
//! shows. Window keys match stream `rate_limit_event` types so `/limits`
//! can merge a live probe with last-observed cache.

use super::{rate_limit::RateLimitState, stream::RateLimitInfo, window_kind::WindowKind};

/// Look-ahead after a header for `% used` / `Resets`.
const HEADER_SCAN_LINES: usize = 6;

struct HeaderHit {
    key: String,
    line: usize,
    start: usize,
    end: usize,
}

/// Parse a reconstructed `/usage` (or `/status` Usage tab) screen.
///
/// Returns `None` when no window with a used-percent could be read.
pub(crate) fn parse_usage_screen(text: &str, now_unix: i64) -> Option<RateLimitState> {
    let lines: Vec<String> = text.lines().map(strip_box_drawing).collect();
    let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let hits = find_headers(&line_refs);
    let mut state = RateLimitState::default();
    for (index, hit) in hits.iter().enumerate() {
        let columns = hits
            .iter()
            .any(|other| other.line == hit.line && other.start != hit.start);
        let region = region_text(&line_refs, hit, hits.get(index + 1), columns);
        if let Some(info) = parse_window_block(&hit.key, &region, now_unix) {
            state.merge_window(super::rate_limit::RateLimitObservation::capture(info));
        }
    }
    if state.is_empty() {
        None
    } else {
        Some(state)
    }
}

/// Window keys the screen names, even when a used% has not painted yet.
pub(crate) fn named_window_keys(text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(strip_box_drawing).collect();
    let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    find_headers(&line_refs)
        .into_iter()
        .map(|hit| hit.key)
        .collect()
}

fn find_headers(lines: &[&str]) -> Vec<HeaderHit> {
    let mut hits = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let lower = text.to_ascii_lowercase();
        push_ascii_hit(
            &mut hits,
            line,
            &lower,
            "current session",
            WindowKind::FiveHour.key(),
        );
        push_ascii_hit(
            &mut hits,
            line,
            &lower,
            "extra usage",
            WindowKind::ExtraUsage.key(),
        );
        hits.extend(week_hits(line, &lower));
    }
    hits.sort_by_key(|hit| (hit.line, hit.start));
    hits
}

fn push_ascii_hit(hits: &mut Vec<HeaderHit>, line: usize, lower: &str, needle: &str, key: &str) {
    let mut from = 0;
    while let Some(offset) = lower[from..].find(needle) {
        let byte = from + offset;
        hits.push(HeaderHit {
            key: key.to_owned(),
            line,
            start: char_count(lower, byte),
            end: char_count(lower, byte + needle.len()),
        });
        from = byte + needle.len();
    }
}

fn week_hits(line: usize, lower: &str) -> Vec<HeaderHit> {
    let needle = "current week (";
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(offset) = lower[from..].find(needle) {
        let start_byte = from + offset;
        let inner_byte = start_byte + needle.len();
        let Some(rel_end) = lower[inner_byte..].find(')') else {
            break;
        };
        let inner = &lower[inner_byte..inner_byte + rel_end];
        let end_byte = inner_byte + rel_end + 1;
        hits.push(HeaderHit {
            key: WindowKind::from_week_inner(inner).key().to_owned(),
            line,
            start: char_count(lower, start_byte),
            end: char_count(lower, end_byte),
        });
        from = end_byte;
    }
    hits
}

fn char_count(text: &str, byte: usize) -> usize {
    text.get(..byte.min(text.len()))
        .map_or(0, |prefix| prefix.chars().count())
}

fn region_text(lines: &[&str], hit: &HeaderHit, next: Option<&HeaderHit>, columns: bool) -> String {
    let mut out = String::new();
    let header_chars: Vec<char> = lines[hit.line].chars().collect();
    let same_line_end = match next {
        Some(next) if next.line == hit.line => next.start,
        _ => header_chars.len(),
    };
    let rest_start = hit.end.min(header_chars.len());
    let rest_end = same_line_end.min(header_chars.len()).max(rest_start);
    out.extend(header_chars[rest_start..rest_end].iter().copied());
    out.push('\n');

    for offset in 1..=HEADER_SCAN_LINES {
        let index = hit.line + offset;
        if index >= lines.len() {
            break;
        }
        if !columns && next.is_some_and(|next| next.line == index) {
            break;
        }
        let chars: Vec<char> = lines[index].chars().collect();
        if columns {
            let end = match next {
                Some(next) if next.line == hit.line => next.start,
                _ => chars.len(),
            };
            let start = hit.start.min(chars.len());
            let end = end.min(chars.len()).max(start);
            out.extend(chars[start..end].iter().copied());
        } else {
            out.extend(chars);
        }
        out.push('\n');
    }
    out
}

fn parse_window_block(window: &str, region: &str, now_unix: i64) -> Option<RateLimitInfo> {
    let mut used_percent = None;
    let mut resets_at = None;
    for line in region.lines() {
        if used_percent.is_none() {
            used_percent = parse_used_percent(line);
        }
        if resets_at.is_none() {
            resets_at = parse_resets_at(line, now_unix);
        }
        if used_percent.is_some() && resets_at.is_some() {
            break;
        }
    }
    let used_percent = used_percent?;
    Some(RateLimitInfo {
        status: None,
        rate_limit_type: Some(window.to_owned()),
        resets_at,
        utilization: Some(used_percent / 100.0),
        overage_status: None,
        overage_resets_at: None,
        is_using_overage: None,
    })
}

fn strip_box_drawing(line: &str) -> String {
    line.trim()
        .trim_start_matches(['│', '|', '╭', '╰', '╮', '╯'])
        .trim_end_matches(['│', '|', '╭', '╰', '╮', '╯'])
        .trim()
        .to_string()
}

fn parse_used_percent(line: &str) -> Option<f64> {
    let lower = line.to_ascii_lowercase();
    let idx = lower.find("% used").or_else(|| lower.find("%used"))?;
    let before = line[..idx].trim_end();
    let token = before.split_whitespace().last()?;
    let value: f64 = token.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(0.0, 100.0))
}

fn parse_resets_at(line: &str, now_unix: i64) -> Option<i64> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("reset") {
        return None;
    }
    if let Some(seconds) = parse_relative_seconds(&lower) {
        return Some(now_unix.saturating_add(seconds));
    }
    parse_local_clock(&lower, now_unix)
}

fn parse_relative_seconds(text: &str) -> Option<i64> {
    let start = text.find(" in ")?;
    let mut seconds = 0_i64;
    let mut saw_unit = false;
    for token in text[start + 4..].split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        let (digits, unit) = split_trailing_unit(token)?;
        let value: i64 = digits.parse().ok()?;
        seconds = seconds.saturating_add(match unit {
            "d" => value.saturating_mul(86_400),
            "h" => value.saturating_mul(3_600),
            "m" => value.saturating_mul(60),
            "s" => value,
            _ => return None,
        });
        saw_unit = true;
    }
    saw_unit.then_some(seconds)
}

fn split_trailing_unit(token: &str) -> Option<(&str, &str)> {
    let split = token.find(|ch: char| ch.is_ascii_alphabetic())?;
    Some((&token[..split], &token[split..]))
}

/// Resolve "Resets 5:30am" / "Resets Sep 5, 8am" against the local clock.
///
/// Time zones match by construction: the probe's `claude` child inherits our
/// environment, and (verified on 2.1.252) the panel renders reset times in
/// the process `TZ` - the same zone `chrono::Local` reads here. The panel's
/// own "(America/Los_Angeles)" label is only touched by the month-anchored
/// day parse, which requires a month name and so never matches digits inside
/// a tz label like "(UTC+10)". Known gap, deliberately unhandled: a reset
/// landing in a DST-ambiguous or skipped local hour makes `single()` return
/// `None`, dropping the countdown for that window rather than showing a time
/// that is wrong by an hour.
///
/// A dated line ("Resets Sep 5, 8am") pins the day-of-month: weekly windows
/// reset at most 7 days out, so the day alone identifies the date within the
/// next 8 days - no month arithmetic needed. Clock-only lines resolve to
/// today-or-tomorrow, the degenerate range of the same search. A named day
/// with no future candidate in range yields `None` (misparse or stale panel)
/// rather than a confidently wrong date.
fn parse_local_clock(text: &str, now_unix: i64) -> Option<i64> {
    use chrono::Datelike;

    let (hour, minute) = parse_12h_time(text)?;
    let now = chrono::DateTime::from_timestamp(now_unix, 0)?
        .with_timezone(&chrono::Local)
        .naive_local();
    let today = now.date();
    let naive_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
    let day = parse_day_of_month(text);
    let range = if day.is_some() { 0..=8 } else { 0..=1 };
    range
        .map(|offset| today + chrono::TimeDelta::days(offset))
        .filter(|date| day.is_none_or(|day| date.day() == day))
        .map(|date| date.and_time(naive_time))
        .find(|candidate| *candidate > now)
        .and_then(local_timestamp)
}

fn local_timestamp(candidate: chrono::NaiveDateTime) -> Option<i64> {
    Some(
        candidate
            .and_local_timezone(chrono::Local)
            .single()?
            .timestamp(),
    )
}

/// Day-of-month anchored on a month name: the `5` in "resets sep 5, 8am".
///
/// The month token is an anchor only - its value is not validated against
/// the resolved date (the day alone pins the date; see [`parse_local_clock`]).
/// Anchoring is what keeps digits elsewhere on the line - clock times, tz
/// labels like "(utc+10)", dollar amounts - from being misread as a day.
fn parse_day_of_month(text: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let (index, month) = MONTHS
        .iter()
        .filter_map(|month| Some((text.find(month)?, month)))
        .min()?;
    let rest = text[index + month.len()..].trim_start();
    let digits = &rest[..rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len())];
    let day: u32 = digits.parse().ok()?;
    (1..=31).contains(&day).then_some(day)
}

fn parse_12h_time(text: &str) -> Option<(u32, u32)> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let hour: u32 = text[start..index].parse().ok()?;
        let mut minute = 0;
        if index < bytes.len() && bytes[index] == b':' {
            index += 1;
            let min_start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if index - min_start != 2 {
                continue;
            }
            minute = text[min_start..index].parse().ok()?;
        }
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let rest = text.get(index..)?;
        let meridiem = if rest.starts_with("am") {
            "am"
        } else if rest.starts_with("pm") {
            "pm"
        } else {
            continue;
        };
        if hour == 0 || hour > 12 || minute > 59 {
            return None;
        }
        let hour24 = match (hour, meridiem) {
            (12, "am") => 0,
            (12, "pm") => 12,
            (h, "pm") => h + 12,
            (h, _) => h,
        };
        return Some((hour24, minute));
    }
    None
}

#[cfg(test)]
#[path = "usage_parse_tests.rs"]
mod tests;
