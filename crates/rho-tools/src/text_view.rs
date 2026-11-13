//! Numbered file views shared by read, write, grep, and every edit surface.
//!
//! Callers supply the header, normally the display path. Line splitting,
//! window selection, and chain snapshots live here.

use std::{borrow::Cow, fmt::Write};

use schemars::JsonSchema;
use serde::Serialize;

mod line_split;
mod window;

pub(crate) use line_split::read_searchable_lines;
pub(crate) use window::{read_text_window, validate_window, CHUNK_SIZE};

#[cfg(test)]
pub(crate) use window::{scan_window_bytes, ScanError};

/// Separator between a 1-indexed line number and the line body.
const LINE_BODY_SEP: char = ':';

/// Soft cap on numbered body rows in write/failure chain snapshots.
const CHAIN_SNAPSHOT_MAX_BODY_LINES: usize = 40;

/// Context lines around op anchors in a failure snapshot.
const CHAIN_SNAPSHOT_CONTEXT_LINES: usize = 2;

/// Head lines kept when a chain snapshot has no focus anchors.
const CHAIN_SNAPSHOT_HEAD_LINES: usize = 28;

/// Tail lines kept after the head so EOF anchors stay chainable on large files.
const CHAIN_SNAPSHOT_TAIL_LINES: usize = 8;

/// Contiguous 1-indexed lines of a text file. Renders as the numbered model
/// view and serializes as `read_file`'s script-facing `data`. Lines borrow the
/// file text until [`Self::render_bounded`] keeps the visible ones.
#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub(crate) struct TextWindow<'t> {
    /// Line number of the first entry in `lines`.
    start_line: usize,
    /// Selected lines without number prefixes or line endings.
    lines: Vec<Cow<'t, str>>,
    /// Content lines in the whole file.
    total_lines: usize,
}

impl<'t> TextWindow<'t> {
    /// Rejects a start past the end; an empty file still allows line 1.
    fn new(
        start_line: usize,
        lines: Vec<Cow<'t, str>>,
        total_lines: usize,
    ) -> Result<Self, String> {
        if start_line > total_lines.max(1) {
            return Err(offset_past_end(start_line, total_lines));
        }
        debug_assert!(start_line + lines.len() <= total_lines + 1);
        Ok(Self {
            start_line,
            lines,
            total_lines,
        })
    }

    /// The model text, cut to `max_bytes` exactly as every tool output is,
    /// and the window trimmed to rows that text shows whole and whose JSON
    /// fits the same budget, so scripts can resume at `start_line + len(lines)`.
    /// `None` when even an empty window's JSON exceeds the budget.
    pub(crate) fn render_bounded(
        self,
        header: &str,
        max_bytes: usize,
    ) -> (String, Option<TextWindow<'static>>) {
        let text = crate::tool::truncate(self.render(header), max_bytes);
        let mut window = TextWindow {
            start_line: self.start_line,
            lines: Vec::new(),
            total_lines: self.total_lines,
        };
        let mut json_used = json_len(&window);
        if json_used > max_bytes {
            return (text, None);
        }
        let mut text_used = header.len();
        for (index, line) in self.lines.into_iter().enumerate() {
            let number = self.start_line + index;
            text_used += 2 + number.to_string().len() + line.len();
            json_used += json_len(&line) + usize::from(index > 0);
            if text_used > max_bytes || json_used > max_bytes {
                break;
            }
            window.lines.push(Cow::Owned(line.into_owned()));
        }
        (text, Some(window))
    }

    /// Numbered rows under `header` (normally the display path), with a
    /// footer when lines outside the window exist.
    pub(crate) fn render(&self, header: &str) -> String {
        if self.lines.is_empty() {
            return header.to_string();
        }
        let mut out = header.to_string();
        for (index, line) in self.lines.iter().enumerate() {
            write!(out, "\n{}{LINE_BODY_SEP}{line}", self.start_line + index)
                .expect("writing to a String cannot fail");
        }
        let end = self.start_line + self.lines.len() - 1;
        if let Some(footer) = window_footer(self.start_line, end, self.total_lines) {
            out.push_str("\n\n");
            out.push_str(&footer);
        }
        out
    }
}

/// Serialized size of plain data; serializing strings and integers cannot fail.
fn json_len<T: Serialize + ?Sized>(value: &T) -> usize {
    crate::json_len(value).expect("text window serializes")
}

/// Select a 1-indexed inclusive `offset`/`limit` window of `text`.
pub(crate) fn select_window(
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<TextWindow<'_>, String> {
    let (start, limit) = validate_window(offset, limit)?;
    let lines = split_content_lines(text);
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(limit.unwrap_or(usize::MAX))
        .map(|line| Cow::Borrowed(*line))
        .collect();
    TextWindow::new(start, selected, lines.len())
}

/// Render a numbered view of `text`; tests compare other read paths to it.
#[cfg(test)]
pub(crate) fn format_numbered_view(
    header: &str,
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    select_window(text, offset, limit).map(|window| window.render(header))
}

fn window_footer(start: usize, end: usize, total: usize) -> Option<String> {
    if start > 1 || end < total {
        Some(format!(
            "[lines {start}-{end} of {total} shown; re-read with a different offset or limit for the rest]"
        ))
    } else {
        None
    }
}

fn offset_past_end(start: usize, total: usize) -> String {
    format!("offset {start} is past the end of the file ({total} line(s))")
}

/// Bounded numbered snapshot after `write` or a failed `edit`.
///
/// Body rows are capped: focus anchors expand locally; otherwise a short
/// head+tail window is used. `header` is normally the display path.
pub(crate) fn format_chain_snapshot(header: &str, text: &str, focus_lines: &[usize]) -> String {
    let lines = split_content_lines(text);
    if lines.is_empty() {
        return header.to_string();
    }
    let total = lines.len();
    let focus = sanitize_focus(focus_lines, total);
    let selected = if focus.is_empty() {
        head_tail_lines(total, CHAIN_SNAPSHOT_HEAD_LINES, CHAIN_SNAPSHOT_TAIL_LINES)
    } else {
        let expanded = expand_focus_lines(&focus, total, CHAIN_SNAPSHOT_CONTEXT_LINES);
        cap_selected_by_hunk(&expanded, CHAIN_SNAPSHOT_MAX_BODY_LINES)
    };
    let footer = chain_footer(&selected, total);
    emit_numbered_body(header, &lines, &selected, footer.as_deref())
}

/// Build the chain truncation footer for a selected/total line count pair.
fn chain_truncation_footer(selected: usize, total: usize) -> String {
    format!("[showing {selected} of {total} lines; re-read with offset/limit for other lines]")
}

fn emit_numbered_body(
    header: &str,
    lines: &[&str],
    selected: &[usize],
    footer: Option<&str>,
) -> String {
    if selected.is_empty() {
        return header.to_string();
    }
    let mut out = header.to_string();
    out.push('\n');
    let mut previous = 0usize;
    for &line_number in selected {
        if previous != 0 && line_number > previous + 1 {
            out.push_str("…\n");
        }
        writeln!(
            out,
            "{line_number}{LINE_BODY_SEP}{}",
            lines[line_number - 1]
        )
        .expect("writing to a String cannot fail");
        previous = line_number;
    }
    out.pop();
    if let Some(footer) = footer {
        out.push_str("\n\n");
        out.push_str(footer);
    }
    out
}

fn sanitize_focus(focus: &[usize], total: usize) -> Vec<usize> {
    let mut focus: Vec<usize> = focus
        .iter()
        .copied()
        .filter(|line| *line >= 1 && *line <= total)
        .collect();
    focus.sort_unstable();
    focus.dedup();
    focus
}

fn head_tail_lines(total: usize, head: usize, tail: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    if total <= head + tail {
        return (1..=total).collect();
    }
    let mut selected = Vec::with_capacity(head + tail);
    selected.extend(1..=head.min(total));
    let tail_start = total.saturating_sub(tail) + 1;
    if tail_start > head {
        selected.extend(tail_start..=total);
    }
    selected
}

fn expand_focus_lines(focus: &[usize], total_lines: usize, context: usize) -> Vec<usize> {
    if total_lines == 0 || focus.is_empty() {
        return Vec::new();
    }
    let mut selected = Vec::new();
    for &line in focus {
        let start = line.saturating_sub(context).max(1);
        let end = (line + context).min(total_lines);
        selected.extend(start..=end);
    }
    selected.sort_unstable();
    selected.dedup();
    selected
}

/// Keep at most `max_lines` rows. Each contiguous hunk gets an equal floor of
/// the budget so a late edit is not starved by an early one.
fn cap_selected_by_hunk(selected: &[usize], max_lines: usize) -> Vec<usize> {
    if selected.len() <= max_lines {
        return selected.to_vec();
    }
    let hunks = split_hunks(selected);
    if hunks.is_empty() {
        return Vec::new();
    }
    let per_hunk = (max_lines / hunks.len()).max(1);
    let mut remaining = max_lines;
    let mut out = Vec::with_capacity(max_lines.min(selected.len()));
    for (index, hunk) in hunks.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        let later = hunks.len() - index - 1;
        // Last hunk takes whatever is left; earlier hunks take the floor share.
        let allow = if later == 0 {
            remaining
        } else {
            per_hunk.min(remaining.saturating_sub(later)).max(1)
        };
        let take = allow.min(hunk.len()).min(remaining);
        out.extend(hunk.iter().copied().take(take));
        remaining -= take;
    }
    out
}

fn split_hunks(selected: &[usize]) -> Vec<Vec<usize>> {
    let mut hunks = Vec::new();
    let mut current = Vec::new();
    let mut previous = 0usize;
    for &line in selected {
        if !current.is_empty() && line > previous + 1 {
            hunks.push(std::mem::take(&mut current));
        }
        current.push(line);
        previous = line;
    }
    if !current.is_empty() {
        hunks.push(current);
    }
    hunks
}

fn chain_footer(selected: &[usize], total: usize) -> Option<String> {
    if selected.is_empty() {
        return None;
    }
    let first = selected[0];
    let last = *selected.last().expect("non-empty");
    if selected.len() == total && first == 1 && last == total {
        return None;
    }
    Some(chain_truncation_footer(selected.len(), total))
}

/// Split file text into addressable content lines iterator.
///
/// A trailing newline does not create an extra blank line. An empty file has
/// zero lines. A file whose sole content is a blank line (single `\n`) has one
/// empty line.
fn iter_content_lines(text: &str) -> impl Iterator<Item = &str> {
    let opt_body = (!text.is_empty()).then(|| text.strip_suffix('\n').unwrap_or(text));
    opt_body
        .into_iter()
        .flat_map(|b| b.split('\n'))
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

/// Split file text into addressable content lines.
///
/// A trailing newline does not create an extra blank line. An empty file has
/// zero lines. A file whose sole content is a blank line (single `\n`) has one
/// empty line.
fn split_content_lines(text: &str) -> Vec<&str> {
    iter_content_lines(text).collect()
}

#[cfg(test)]
#[path = "text_view_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "text_view/window_tests.rs"]
mod window_tests;
