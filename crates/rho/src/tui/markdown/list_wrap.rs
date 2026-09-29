//! Hanging-indent wrapping for markdown list items.
//!
//! A wrapped list item keeps its marker on the first row and hangs later rows
//! under the item text. List-ness is read from the markdown *source* line, so
//! the final render and streaming bounds agree (and `` `-` x`` stays prose).

use std::ops::Range;

use super::{
    display_width, wrap_line_at_whitespace_ranges,
    wrap_line_at_whitespace_ranges_with_protected_prefix,
};

/// Caller-chosen wrap behavior for one rendered markdown line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WrapPolicy {
    /// Every row starts at column 0 (headings, table cells).
    Flush,
    /// List item whose marker (with separator) is `hang` columns wide.
    ListItem { hang: usize },
    /// Tail of a list item whose marker row is already committed; every row
    /// is a continuation row hung `hang` columns in.
    ContinuedListItem { hang: usize },
}

impl WrapPolicy {
    /// Policy for a paragraph whose markdown source is `source_line`.
    pub(super) fn for_paragraph(source_line: &str) -> Self {
        match markdown_list_marker_width(source_line) {
            0 => Self::Flush,
            hang => Self::ListItem { hang },
        }
    }

    /// Resolve at `width`. Hangs that leave no room for text wrap flush.
    pub(super) fn resolve(self, width: usize) -> ContinuationIndent {
        let fit = |hang: usize| (hang > 0 && hang < width).then_some(hang);
        match self {
            Self::Flush => ContinuationIndent::Flush,
            Self::ListItem { hang } => {
                fit(hang).map_or(ContinuationIndent::Flush, ContinuationIndent::Hang)
            }
            Self::ContinuedListItem { hang } => {
                fit(hang).map_or(ContinuationIndent::Flush, ContinuationIndent::Continued)
            }
        }
    }
}

/// Resolved indent for the rows of one wrapped line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContinuationIndent {
    Flush,
    /// First row spans the full width; later rows hang this many columns in.
    Hang(usize),
    /// Every row hangs this many columns in.
    Continued(usize),
}

impl ContinuationIndent {
    /// Columns of padding before the row that starts at byte `row_start`.
    pub(super) fn row_indent(self, row_start: usize) -> usize {
        match self {
            Self::Flush => 0,
            Self::Hang(_) if row_start == 0 => 0,
            Self::Hang(hang) | Self::Continued(hang) => hang,
        }
    }
}

/// Where pending stream text starts relative to its markdown source line.
///
/// Streams commit long lines in pieces, so pending text can begin in the middle
/// of a list item whose marker is already in the transcript. Its first line
/// then wraps as hung continuation rows, matching the final render whenever the
/// committed piece ended on a row boundary. Lines with inline markup may commit
/// a partial last row; the next drain re-renders the whole line, so the preview
/// row split lasts one tick.
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
        match WrapPolicy::for_paragraph(&emitted[line_start..]) {
            WrapPolicy::ListItem { hang } => Self::ContinuesListItem { hang },
            WrapPolicy::Flush | WrapPolicy::ContinuedListItem { .. } => Self::Fresh,
        }
    }

    /// Policy for the first pending line, or `None` when it starts a fresh
    /// source line and goes through normal block detection.
    pub(super) fn continued_policy(self) -> Option<WrapPolicy> {
        match self {
            Self::Fresh => None,
            Self::ContinuesListItem { hang } => Some(WrapPolicy::ContinuedListItem { hang }),
        }
    }
}

/// Covering soft-wrap ranges for one rendered markdown line.
///
/// Hung rows wrap at `width - hang`; callers pad them by
/// [`ContinuationIndent::row_indent`]. Streaming bounds share these ranges to
/// stay in lockstep with the final render.
pub(super) fn wrap_markdown_line_ranges(
    line: &str,
    width: usize,
    indent: ContinuationIndent,
) -> Vec<Range<usize>> {
    let protected_prefix_end = markdown_list_body_start(line).unwrap_or_default();
    match indent {
        ContinuationIndent::Flush => {
            wrap_line_at_whitespace_ranges_with_protected_prefix(line, width, protected_prefix_end)
        }
        ContinuationIndent::Hang(hang) => {
            let first = wrap_line_at_whitespace_ranges_with_protected_prefix(
                line,
                width,
                protected_prefix_end,
            )
            .into_iter()
            .next()
            .unwrap_or(0..line.len());
            let rest_start = first.end;
            let mut ranges = vec![first];
            if rest_start < line.len() {
                ranges.extend(
                    wrap_line_at_whitespace_ranges(&line[rest_start..], width - hang)
                        .into_iter()
                        .map(|range| range.start + rest_start..range.end + rest_start),
                );
            }
            ranges
        }
        ContinuationIndent::Continued(hang) => wrap_line_at_whitespace_ranges(line, width - hang),
    }
}

/// Display width of `line`'s list marker and separator, or 0 when `line` is
/// not a list item.
fn markdown_list_marker_width(line: &str) -> usize {
    markdown_list_body_start(line).map_or(0, |start| display_width(&line[..start]))
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
