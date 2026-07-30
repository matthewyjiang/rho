//! Locate patch context within file lines.
//!
//! Adapted from the Apache-2.0 codex-rs apply-patch crate.

/// Find `pattern` in `lines` at or after `start`.
///
/// Match strictness decreases: exact, trailing-whitespace, trim, then Unicode
/// punctuation normalisation. When `eof` is true, try the final window first,
/// then fall back to scanning from `start`.
pub(super) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }

    if eof {
        let end_start = lines.len() - pattern.len();
        if let Some(idx) = search_from(lines, pattern, end_start) {
            return Some(idx);
        }
    }
    search_from(lines, pattern, start)
}

fn search_from(lines: &[String], pattern: &[String], start: usize) -> Option<usize> {
    find_with(lines, pattern, start, |left, right| left == right)
        .or_else(|| {
            find_with(lines, pattern, start, |left, right| {
                left.trim_end() == right.trim_end()
            })
        })
        .or_else(|| {
            find_with(lines, pattern, start, |left, right| {
                left.trim() == right.trim()
            })
        })
        .or_else(|| {
            find_with(lines, pattern, start, |left, right| {
                normalise(left) == normalise(right)
            })
        })
}

fn find_with(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eq: impl Fn(&str, &str) -> bool,
) -> Option<usize> {
    (start..=lines.len().saturating_sub(pattern.len())).find(|&index| {
        pattern
            .iter()
            .enumerate()
            .all(|(offset, pat)| eq(&lines[index + offset], pat))
    })
}

fn normalise(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
#[path = "seek_sequence_tests.rs"]
mod tests;
