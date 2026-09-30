//! Hanging-indent wrapping for markdown list items.
//!
//! A wrapped list item keeps its marker on the first row and hangs later rows
//! under the item text. Whether a line hangs is read from its markdown source,
//! so `` `-` x`` does not hang; the final render and streaming bounds share
//! [`wrap_markdown_line_ranges`] to stay in lockstep.

use std::ops::Range;

use super::{
    display_width, wrap_line_at_whitespace_ranges,
    wrap_line_at_whitespace_ranges_with_protected_prefix,
};

/// Resolved indent for the rows of one wrapped markdown line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContinuationIndent {
    /// Every row starts at column 0 (prose, headings, table cells).
    Flush,
    /// List item: the first row spans the full width; later rows hang this
    /// many columns in, under the item text.
    Hang(usize),
    /// Tail of a list item whose marker row is already committed: every row
    /// is a continuation row, hung this many columns in.
    Continued(usize),
}

impl ContinuationIndent {
    /// Indent for a paragraph whose markdown source is `source_line`.
    pub(super) fn list_item(source_line: &str, width: usize) -> Self {
        list_hang(source_line)
            .and_then(|hang| fit_hang(hang, width))
            .map_or(Self::Flush, Self::Hang)
    }

    /// Indent for the tail of a committed list item with marker width `hang`.
    fn continued(hang: usize, width: usize) -> Self {
        fit_hang(hang, width).map_or(Self::Flush, Self::Continued)
    }

    /// Columns of padding before the row that starts at byte `row_start`.
    pub(super) fn row_indent(self, row_start: usize) -> usize {
        match self {
            Self::Flush => 0,
            Self::Hang(_) if row_start == 0 => 0,
            Self::Hang(hang) | Self::Continued(hang) => hang,
        }
    }
}

/// `hang` when continuation rows still leave room for text at `width`.
fn fit_hang(hang: usize, width: usize) -> Option<usize> {
    (hang < width).then_some(hang)
}

/// Where pending stream text starts relative to its markdown source line.
///
/// Streams commit long lines in pieces, so pending text can begin in the middle
/// of a list item whose marker is already in the transcript. Its first line
/// then wraps as hung continuation rows, matching the final render whenever the
/// committed piece ended on a row boundary. Inline spans that cannot be split
/// at that boundary stay in the live preview until a safe cut is available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) enum StreamLineStart {
    /// Pending text begins its own line, or continues a line that wraps flush.
    #[default]
    Fresh,
    /// Pending text continues a committed list item with this marker width.
    ContinuesListItem { hang: usize },
}

impl StreamLineStart {
    /// Line start for text appended after `emitted`. `in_code_block` is the
    /// fence state after `emitted`; code lines hard-wrap without a hang.
    pub(in crate::tui) fn after(emitted: &str, in_code_block: bool) -> Self {
        if in_code_block {
            return Self::Fresh;
        }
        let line_start = emitted
            .rfind('\n')
            .map_or(0, |index| index + '\n'.len_utf8());
        list_hang(&emitted[line_start..])
            .map_or(Self::Fresh, |hang| Self::ContinuesListItem { hang })
    }

    /// Indent for the first pending line at `width`, or `None` when it starts
    /// a fresh source line and goes through normal block detection.
    pub(super) fn continued_indent(self, width: usize) -> Option<ContinuationIndent> {
        match self {
            Self::Fresh => None,
            Self::ContinuesListItem { hang } => Some(ContinuationIndent::continued(hang, width)),
        }
    }
}

/// Soft-wrap ranges for one rendered markdown line.
///
/// Hung rows wrap at `width - hang` and skip leading break whitespace, so a
/// continued tail wraps exactly like the rows after an item's first row; a
/// whitespace-only tail has no rows. Callers pad rows by
/// [`ContinuationIndent::row_indent`].
pub(super) fn wrap_markdown_line_ranges(
    line: &str,
    width: usize,
    indent: ContinuationIndent,
) -> Vec<Range<usize>> {
    let protected = |line: &str| {
        let protected_prefix_end = markdown_list_body_start(line).unwrap_or_default();
        wrap_line_at_whitespace_ranges_with_protected_prefix(line, width, protected_prefix_end)
    };
    match indent {
        ContinuationIndent::Flush => protected(line),
        ContinuationIndent::Hang(hang) => {
            let mut ranges = protected(line);
            ranges.truncate(1);
            let rest_start = ranges.first().map_or(0, |first| first.end);
            ranges.extend(hung_ranges(line, rest_start, width - hang));
            ranges
        }
        ContinuationIndent::Continued(hang) => hung_ranges(line, 0, width - hang),
    }
}

/// Ranges for `line[start..]` wrapped at `width`, skipping leading whitespace.
fn hung_ranges(line: &str, start: usize, width: usize) -> Vec<Range<usize>> {
    let rest = &line[start..];
    let start = start + (rest.len() - rest.trim_start().len());
    if start == line.len() {
        return Vec::new();
    }
    wrap_line_at_whitespace_ranges(&line[start..], width)
        .into_iter()
        .map(|range| range.start + start..range.end + start)
        .collect()
}

/// Display width of a list item's marker and separator, or `None` when
/// `line` is not a list item.
fn list_hang(line: &str) -> Option<usize> {
    markdown_list_body_start(line).map(|start| display_width(&line[..start]))
}

fn markdown_list_body_start(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches(char::is_whitespace);
    let leading_whitespace_len = line.len() - trimmed.len();
    let marker_len = trimmed.find(char::is_whitespace)?;
    let marker = &trimmed[..marker_len];
    let is_list_marker = matches!(marker, "-" | "+" | "*")
        || marker.strip_suffix(['.', ')']).is_some_and(|digits| {
            (1..=9).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !is_list_marker {
        return None;
    }

    let separator_len = trimmed[marker_len..]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .map(char::len_utf8)
        .sum::<usize>();
    let body_start = leading_whitespace_len + marker_len + separator_len;
    (body_start < line.len()).then_some(body_start)
}
