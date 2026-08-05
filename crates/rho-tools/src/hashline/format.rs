//! Hashline display format: file snapshot tags and numbered line views.
//!
//! Tags are harness-local fingerprints of normalized file text. They are not a
//! global content-addressed store; any reader that sees the same bytes mints the
//! same tag, and editors reject a tag when the live file no longer matches.

/// Number of uppercase hex digits in a file snapshot tag.
///
/// Four hex digits (low 16 bits of FNV-1a) match the oh-my-pi wire format and
/// keep headers cheap. The tag is a freshness lock: after the live file drifts,
/// `edit` fails closed and returns a new live snapshot to copy.
pub(crate) const FILE_HASH_LENGTH: usize = 4;

/// Separator between a path and its snapshot tag inside a section header.
pub(crate) const FILE_HASH_SEP: char = '#';

/// Separator between a 1-indexed line number and the line body.
pub(crate) const LINE_BODY_SEP: char = ':';

/// Compute the snapshot tag for normalized file text.
pub(crate) fn compute_file_hash(text: &str) -> String {
    let digest = fnv1a32(normalize_for_hash(text).as_bytes()) & 0xFFFF;
    format!("{digest:04X}")
}

/// Format a section header `[path#TAG]`.
pub(crate) fn format_header(path: &str, file_hash: &str) -> String {
    format!("[{path}{FILE_HASH_SEP}{file_hash}]")
}

/// Format one numbered content line as `N:text` (no trailing newline).
pub(crate) fn format_numbered_line(line_number: usize, line: &str) -> String {
    format!("{line_number}{LINE_BODY_SEP}{line}")
}

/// Context lines kept on each side of a post-edit focus line.
///
/// Three lines is enough to re-anchor a follow-up PUT/CUT without dumping the
/// whole file; measured against typical single-hunk agent edits.
const POST_EDIT_CONTEXT_LINES: usize = 3;

/// Soft cap on numbered body rows in a post-edit chain preview.
///
/// Sized so multi-hunk previews stay scannable while still leaving room for a
/// few context lines around each focus region.
const POST_EDIT_MAX_BODY_LINES: usize = 40;

/// Soft cap on numbered body rows in write/failure chain snapshots.
///
/// write_file and failed edit must return a usable `[path#TAG]` without dumping
/// whole files into the model context. Full bodies stay on `read_file`.
const CHAIN_SNAPSHOT_MAX_BODY_LINES: usize = 40;

/// Context lines around op anchors in a failure snapshot.
const CHAIN_SNAPSHOT_CONTEXT_LINES: usize = 2;

/// Head lines kept when a chain snapshot has no focus anchors.
const CHAIN_SNAPSHOT_HEAD_LINES: usize = 28;

/// Tail lines kept after the head so EOF anchors stay chainable on large files.
const CHAIN_SNAPSHOT_TAIL_LINES: usize = 8;

/// How to pick body rows for a chainable numbered snapshot.
enum LineSelection<'a> {
    Focus {
        lines: &'a [usize],
        context: usize,
        max_body: usize,
    },
    HeadTail {
        head: usize,
        tail: usize,
    },
}

/// Footer wording for a numbered snapshot.
enum SnapshotFooter {
    ReadWindow {
        start: usize,
        end: usize,
        total: usize,
    },
    PostEdit {
        first: usize,
        last: usize,
        total: usize,
    },
    ChainPartial {
        shown: usize,
        total: usize,
    },
}

impl SnapshotFooter {
    fn render(&self) -> String {
        match self {
            Self::ReadWindow { start, end, total } => format!(
                "[lines {start}-{end} of {total} shown; re-read with a different offset or limit for the rest]"
            ),
            Self::PostEdit { first, last, total } => format!(
                "[post-edit lines {first}-{last} of {total} shown around changes; re-read for other lines]"
            ),
            Self::ChainPartial { shown, total } => {
                format!("[showing {shown} of {total} lines; re-read with offset/limit for other lines]")
            }
        }
    }
}

/// Render a hashline view of `text` for `display_path`.
///
/// `offset`/`limit` select a 1-indexed inclusive window of lines. The header
/// always carries the full-file tag so later edits can validate the snapshot.
pub(crate) fn format_hashline_view(
    display_path: &str,
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    if offset == Some(0) {
        return Err("offset must be greater than 0".into());
    }
    if limit == Some(0) {
        return Err("limit must be greater than 0".into());
    }

    let lines = split_content_lines(text);
    let total = lines.len();
    let start = offset.unwrap_or(1);
    let hash = compute_file_hash(text);
    let header = format_header(display_path, &hash);
    if total == 0 {
        if start > 1 {
            return Err(format!(
                "offset {start} is past the end of the file (0 line(s))"
            ));
        }
        return Ok(header);
    }
    if start > total {
        return Err(format!(
            "offset {start} is past the end of the file ({total} line(s))"
        ));
    }

    let end = match limit {
        Some(limit) => start.saturating_add(limit).saturating_sub(1).min(total),
        None => total,
    };
    let selected: Vec<usize> = (start..=end).collect();
    let footer = if start > 1 || end < total {
        Some(SnapshotFooter::ReadWindow { start, end, total })
    } else {
        None
    };
    Ok(emit_numbered_body(&header, &lines, &selected, footer))
}

/// Render a chainable post-edit hashline preview for `new_text`.
///
/// The header carries the post-edit tag. Body rows use **post-edit** line
/// numbers around `focus_lines` (from apply) so a follow-up `edit` can copy them
/// without a full re-read. Large unchanged spans collapse to `…`.
pub(crate) fn format_post_edit_preview(
    display_path: &str,
    new_text: &str,
    focus_lines: &[usize],
) -> String {
    format_numbered(
        display_path,
        new_text,
        LineSelection::Focus {
            lines: focus_lines,
            context: POST_EDIT_CONTEXT_LINES,
            max_body: POST_EDIT_MAX_BODY_LINES,
        },
        SnapshotKind::PostEdit,
    )
}

/// Bounded hashline snapshot for chaining after `write_file` or a failed `edit`.
///
/// Always includes the full-file TAG. Body rows are capped so failures do not
/// bloat context: focus anchors (when provided) expand locally; otherwise a
/// short head+tail window is used.
pub(crate) fn format_chain_snapshot(
    display_path: &str,
    text: &str,
    focus_lines: &[usize],
) -> String {
    let selection = if focus_lines.is_empty() {
        LineSelection::HeadTail {
            head: CHAIN_SNAPSHOT_HEAD_LINES,
            tail: CHAIN_SNAPSHOT_TAIL_LINES,
        }
    } else {
        LineSelection::Focus {
            lines: focus_lines,
            context: CHAIN_SNAPSHOT_CONTEXT_LINES,
            max_body: CHAIN_SNAPSHOT_MAX_BODY_LINES,
        }
    };
    format_numbered(display_path, text, selection, SnapshotKind::Chain)
}

/// Which product a numbered snapshot is for (footer and empty-focus policy).
#[derive(Clone, Copy)]
enum SnapshotKind {
    PostEdit,
    Chain,
}

fn format_numbered(
    display_path: &str,
    text: &str,
    selection: LineSelection<'_>,
    kind: SnapshotKind,
) -> String {
    let tag = compute_file_hash(text);
    let header = format_header(display_path, &tag);
    let lines = split_content_lines(text);
    if lines.is_empty() {
        return header;
    }

    let total = lines.len();
    let selected = match selection {
        LineSelection::Focus {
            lines: focus,
            context,
            max_body,
        } => {
            let mut focus = focus.to_vec();
            focus.retain(|line| *line >= 1 && *line <= total);
            focus.sort_unstable();
            focus.dedup();
            if focus.is_empty() {
                match kind {
                    SnapshotKind::PostEdit => {
                        focus.push(1);
                        let expanded = expand_focus_lines(&focus, total, context);
                        cap_selected_by_hunk(&expanded, max_body)
                    }
                    SnapshotKind::Chain => {
                        head_tail_lines(total, CHAIN_SNAPSHOT_HEAD_LINES, CHAIN_SNAPSHOT_TAIL_LINES)
                    }
                }
            } else {
                let expanded = expand_focus_lines(&focus, total, context);
                cap_selected_by_hunk(&expanded, max_body)
            }
        }
        LineSelection::HeadTail { head, tail } => head_tail_lines(total, head, tail),
    };
    if selected.is_empty() {
        return header;
    }

    let first = selected[0];
    let last = *selected.last().expect("non-empty selection");
    let footer = match kind {
        SnapshotKind::PostEdit => {
            if first > 1 || last < total {
                Some(SnapshotFooter::PostEdit { first, last, total })
            } else {
                None
            }
        }
        SnapshotKind::Chain => {
            let showed_all = selected.len() == total && first == 1 && last == total;
            if showed_all {
                None
            } else {
                Some(SnapshotFooter::ChainPartial {
                    shown: selected.len(),
                    total,
                })
            }
        }
    };
    emit_numbered_body(&header, &lines, &selected, footer)
}

fn emit_numbered_body(
    header: &str,
    lines: &[&str],
    selected: &[usize],
    footer: Option<SnapshotFooter>,
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
        out.push_str(&footer.render());
    }
    out
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
    if total_lines == 0 {
        return Vec::new();
    }
    let mut selected = Vec::new();
    for &line in focus {
        let start = line.saturating_sub(context).max(1);
        let end = (line + context).min(total_lines);
        for candidate in start..=end {
            selected.push(candidate);
        }
    }
    selected.sort_unstable();
    selected.dedup();
    selected
}

/// Keep at most `max_lines` rows, spreading the budget across contiguous hunks
/// so a late edit is not starved by an early one.
fn cap_selected_by_hunk(selected: &[usize], max_lines: usize) -> Vec<usize> {
    if selected.len() <= max_lines {
        return selected.to_vec();
    }
    let hunks = split_hunks(selected);
    if hunks.is_empty() {
        return Vec::new();
    }

    // Give each hunk a floor share, then walk hunks in order filling remaining
    // budget so small early hunks do not eat the whole preview.
    let per_hunk = (max_lines / hunks.len()).max(1);
    let mut remaining = max_lines;
    let mut out = Vec::with_capacity(max_lines.min(selected.len()));
    for (index, hunk) in hunks.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        let later = hunks.len() - index - 1;
        let reserved_for_later = later; // at least one line per remaining hunk when possible
        let allow = if later == 0 {
            remaining
        } else {
            remaining
                .saturating_sub(reserved_for_later)
                .min(per_hunk.max(remaining / (later + 1)))
                .max(1)
                .min(remaining)
        };
        let take = allow.min(hunk.len());
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

/// Split file text into addressable content lines.
///
/// A trailing newline does not create an extra blank line. An empty file has
/// zero lines. A file whose sole content is a blank line (single `\n`) has one
/// empty line.
pub(crate) fn split_content_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    // Preserve CRLF bodies as line text without the CR so numbered views and
    // hash anchors stay stable across newline styles.
    for line in &mut lines {
        *line = line.strip_suffix('\r').unwrap_or(line);
    }
    lines
}

/// Detect the dominant end-of-line sequence in `text`.
pub(crate) fn detect_eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.bytes().filter(|byte| *byte == b'\n').count() - crlf;
    let cr = text
        .as_bytes()
        .iter()
        .enumerate()
        .filter(|(index, byte)| **byte == b'\r' && text.as_bytes().get(index + 1) != Some(&b'\n'))
        .count();
    if cr > crlf && cr > lf {
        "\r"
    } else if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// True when `text` ends with a newline sequence.
pub(crate) fn has_trailing_newline(text: &str) -> bool {
    text.ends_with('\n') || text.ends_with('\r')
}

/// Strip trailing spaces/tabs/CR from each line before hashing so display trim
/// and CRLF endings do not invalidate a tag.
pub(crate) fn normalize_for_hash(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let ends_with_newline = text.ends_with('\n');
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_end_matches([' ', '\t', '\r']);
        out.push_str(trimmed);
    }
    if ends_with_newline && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn fnv1a32(data: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in data {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
