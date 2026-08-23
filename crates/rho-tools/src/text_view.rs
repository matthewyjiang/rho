//! Numbered file views shared by every edit surface.
//!
//! Callers supply the header. Hashline wraps that header as `[path#TAG]`;
//! apply_patch and str_replace keep the bare path. Line splitting, window
//! selection, and chain snapshots live here so hashline is not the generic
//! renderer.

mod line_split;
mod window;

pub(crate) use line_split::LineFingerprint;
pub(crate) use window::{read_searchable_lines, read_text_window, validate_window, CHUNK_SIZE};

#[cfg(test)]
pub(crate) use window::{format_window_bytes, ScanError};

/// Separator between a 1-indexed line number and the line body.
pub(crate) const LINE_BODY_SEP: char = ':';

/// Soft cap on numbered body rows in write/failure chain snapshots.
const CHAIN_SNAPSHOT_MAX_BODY_LINES: usize = 40;

/// Context lines around op anchors in a failure snapshot.
const CHAIN_SNAPSHOT_CONTEXT_LINES: usize = 2;

/// Head lines kept when a chain snapshot has no focus anchors.
pub(crate) const CHAIN_SNAPSHOT_HEAD_LINES: usize = 28;

/// Tail lines kept after the head so EOF anchors stay chainable on large files.
pub(crate) const CHAIN_SNAPSHOT_TAIL_LINES: usize = 8;

/// Format one numbered content line as `N:text` (no trailing newline).
pub(crate) fn format_numbered_line(line_number: usize, line: &str) -> String {
    format!("{line_number}{LINE_BODY_SEP}{line}")
}

/// Render a numbered view of `text`.
///
/// `offset`/`limit` select a 1-indexed inclusive window of lines. Callers that
/// need a snapshot tag pass a `[path#TAG]` header; everyone else passes the
/// display path.
pub(crate) fn format_numbered_view(
    header: &str,
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let (start, limit) = validate_window(offset, limit)?;
    let lines = split_content_lines(text);
    let total = lines.len();
    if total == 0 {
        if start > 1 {
            return Err(offset_past_end(start, 0));
        }
        return Ok(header.to_string());
    }
    if start > total {
        return Err(offset_past_end(start, total));
    }

    let end = match limit {
        Some(limit) => start.saturating_add(limit).saturating_sub(1).min(total),
        None => total,
    };
    let selected: Vec<usize> = (start..=end).collect();
    let footer = window_footer(start, end, total);
    Ok(emit_numbered_body(
        header,
        &lines,
        &selected,
        footer.as_deref(),
    ))
}

pub(crate) fn window_footer(start: usize, end: usize, total: usize) -> Option<String> {
    if start > 1 || end < total {
        Some(format!(
            "[lines {start}-{end} of {total} shown; re-read with a different offset or limit for the rest]"
        ))
    } else {
        None
    }
}

pub(crate) fn offset_past_end(start: usize, total: usize) -> String {
    format!("offset {start} is past the end of the file ({total} line(s))")
}

/// Bounded numbered snapshot after `write` or a failed `edit`.
///
/// Body rows are capped: focus anchors expand locally; otherwise a short
/// head+tail window is used. Pass a tagged header or the display path.
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
pub(crate) fn chain_truncation_footer(selected: usize, total: usize) -> String {
    format!("[showing {selected} of {total} lines; re-read with offset/limit for other lines]")
}

pub(crate) fn emit_numbered_body(
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
        out.push_str(&format_numbered_line(line_number, lines[line_number - 1]));
        out.push('\n');
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

pub(crate) fn expand_focus_lines(
    focus: &[usize],
    total_lines: usize,
    context: usize,
) -> Vec<usize> {
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
pub(crate) fn cap_selected_by_hunk(selected: &[usize], max_lines: usize) -> Vec<usize> {
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
pub(crate) fn iter_content_lines(text: &str) -> impl Iterator<Item = &str> {
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
pub(crate) fn split_content_lines(text: &str) -> Vec<&str> {
    iter_content_lines(text).collect()
}

#[cfg(test)]
#[path = "text_view_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "text_view/window_tests.rs"]
mod window_tests;
