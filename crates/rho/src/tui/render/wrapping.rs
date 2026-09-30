//! Grapheme-preserving wrapping shared by composer and transcript rendering.

use unicode_segmentation::UnicodeSegmentation;

use super::display_width;

/// Word-wrap a line for display. Break-boundary whitespace is collapsed so
/// continuation rows are not indented; pure whitespace lines still wrap.
pub(in crate::tui) fn wrap_line_at_whitespace(line: &str, width: usize) -> Vec<&str> {
    soft_wrap_visible_ranges(line, wrap_line_at_whitespace_ranges(line, width))
        .map(|range| &line[range])
        .collect()
}

/// Covering soft-wrap ranges: every source byte belongs to exactly one range.
///
/// Display callers that should not indent continuations must run the result
/// through [`soft_wrap_visible_ranges`]. Composer lockstep uses the covering
/// ranges directly so break spaces stay addressable.
pub(in crate::tui) fn wrap_line_at_whitespace_ranges(
    line: &str,
    width: usize,
) -> Vec<std::ops::Range<usize>> {
    wrap_line_at_whitespace_ranges_with_protected_prefix(line, width, 0)
}

/// Wrap at whitespace without allowing the first break to strand a semantic prefix.
///
/// `protected_prefix_end` is a byte offset whose preceding whitespace cannot be
/// used as the first wrap point. If the following token overflows, the first
/// line is filled to `width` instead. An indivisible grapheme wider than `width`
/// occupies its own row rather than losing source text or splitting the cluster.
pub(in crate::tui) fn wrap_line_at_whitespace_ranges_with_protected_prefix(
    line: &str,
    width: usize,
    protected_prefix_end: usize,
) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    if line.is_empty() {
        return std::iter::once(0..0).collect();
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    while start < line.len() {
        let mut count = 0usize;
        let mut last_fitting_split = None;
        let mut whitespace_break = None;
        let mut saw_non_whitespace = false;
        let mut overflow = false;
        let mut prefer_width_split = false;

        for (relative_index, grapheme) in line[start..].grapheme_indices(true) {
            let grapheme_width = display_width(grapheme);
            let is_whitespace = grapheme.chars().all(char::is_whitespace);
            if count > 0 && count + grapheme_width > width {
                overflow = true;
                prefer_width_split = is_whitespace;
                break;
            }

            count += grapheme_width;
            let next = start + relative_index + grapheme.len();
            last_fitting_split = Some(next);
            if is_whitespace {
                if saw_non_whitespace {
                    whitespace_break = Some(next);
                }
            } else {
                saw_non_whitespace = true;
            }
        }

        if !overflow {
            ranges.push(start..line.len());
            break;
        }

        let split = if prefer_width_split
            || (start == 0 && whitespace_break.is_some_and(|split| split <= protected_prefix_end))
        {
            last_fitting_split.expect("overflow requires a fitting split")
        } else {
            whitespace_break
                .filter(|split| *split > start)
                .unwrap_or_else(|| last_fitting_split.expect("overflow requires a fitting split"))
        };
        ranges.push(start..split);
        start = split;
    }

    ranges
}

/// Collapse break-boundary whitespace from covering soft-wrap ranges for display.
///
/// After a range that contained non-whitespace, leading whitespace on the next
/// range is break padding and is dropped so the continuation is not indented.
/// Pure whitespace segments keep their spaces so blank padding still wraps.
pub(in crate::tui) fn soft_wrap_visible_ranges<'a>(
    line: &'a str,
    ranges: impl IntoIterator<Item = std::ops::Range<usize>> + 'a,
) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
    let mut prev_had_non_whitespace = false;
    ranges.into_iter().filter_map(move |range| {
        let end = range.end;
        let mut start = range.start;
        if prev_had_non_whitespace {
            for grapheme in line[start..end].graphemes(true) {
                if !grapheme.chars().all(char::is_whitespace) {
                    break;
                }
                start += grapheme.len();
            }
            if start >= end {
                return None;
            }
        }
        prev_had_non_whitespace = line[start..end].chars().any(|ch| !ch.is_whitespace());
        Some(start..end)
    })
}

/// Hard-wrap `text` into display-width columns as byte ranges into `text`.
///
/// Empty input yields one empty range. Grapheme clusters are never split, even
/// if one is wider than `width`. A chunk that exactly fills `width` breaks after it.
pub(in crate::tui) fn hard_wrap_ranges(text: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![std::ops::Range { start: 0, end: 0 }];
    }
    let mut ranges = Vec::new();
    let mut chunk_start = 0usize;
    let mut current_width = 0usize;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let grapheme_width = display_width(grapheme);
        if current_width > 0 && current_width + grapheme_width > width {
            ranges.push(chunk_start..offset);
            chunk_start = offset;
            current_width = 0;
        }
        current_width += grapheme_width;
        if current_width >= width {
            let end = offset + grapheme.len();
            ranges.push(chunk_start..end);
            chunk_start = end;
            current_width = 0;
        }
    }
    if chunk_start < text.len() {
        ranges.push(chunk_start..text.len());
    }
    ranges
}

pub(in crate::tui) fn wrap_line_hard(line: &str, width: usize) -> Vec<&str> {
    hard_wrap_ranges(line, width)
        .into_iter()
        .map(|range| &line[range])
        .collect()
}

#[cfg(test)]
#[path = "wrapping_tests.rs"]
mod tests;
