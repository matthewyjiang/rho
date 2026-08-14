//! Hashline display format: file snapshot tags and numbered line views.
//!
//! Tags are harness-local fingerprints of normalized file text. They are not a
//! global content-addressed store; any reader that sees the same bytes mints the
//! same tag, and editors reject a tag when the live file no longer matches.
//!
//! Snapshots share one pipeline: callers pick line numbers, then
//! [`emit_numbered_body`] renders header + rows + optional footer.

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
    let mut hasher = Fnv1a32::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            hasher.write(b"\n");
        }
        let trimmed = line.trim_end_matches([' ', '\t', '\r']);
        hasher.write(trimmed.as_bytes());
    }
    let digest = hasher.finish() & 0xFFFF;
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

/// Wire locator for a PUT replace/single-line range (no trailing colon).
pub(super) fn format_put_locator(start: usize, end: usize) -> String {
    if start == end {
        format!("PUT {start}")
    } else {
        format!("PUT {start}.={end}")
    }
}

/// Wire locator for a CUT range (no trailing colon).
pub(super) fn format_cut_locator(start: usize, end: usize) -> String {
    if start == end {
        format!("CUT {start}")
    } else {
        format!("CUT {start}.={end}")
    }
}

/// Context lines kept on each side of a post-edit focus line.
const POST_EDIT_CONTEXT_LINES: usize = 3;

/// Soft cap on numbered body rows in a post-edit chain preview.
const POST_EDIT_MAX_BODY_LINES: usize = 40;

/// Soft cap on numbered body rows in write/failure chain snapshots.
const CHAIN_SNAPSHOT_MAX_BODY_LINES: usize = 40;

/// Context lines around op anchors in a failure snapshot.
const CHAIN_SNAPSHOT_CONTEXT_LINES: usize = 2;

/// Head lines kept when a chain snapshot has no focus anchors.
pub(crate) const CHAIN_SNAPSHOT_HEAD_LINES: usize = 28;

/// Tail lines kept after the head so EOF anchors stay chainable on large files.
pub(crate) const CHAIN_SNAPSHOT_TAIL_LINES: usize = 8;

/// Marker text embedded in structural post-edit footers (tests assert against this).
pub(crate) const STRUCTURAL_EDIT_FOOTER_MARKER: &str = "structural edit";

/// Marker that structural previews omit numbered chainable body lines.
pub(crate) const STRUCTURAL_NO_CHAIN_MARKER: &str = "no chainable body lines";

/// Build the chain truncation footer for a selected/total line count pair.
pub(crate) fn chain_truncation_footer(selected: usize, total: usize) -> String {
    format!("[showing {selected} of {total} lines; re-read with offset/limit for other lines]")
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
    let header = format_header(display_path, &compute_file_hash(text));
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
        Some(format!(
            "[lines {start}-{end} of {total} shown; re-read with a different offset or limit for the rest]"
        ))
    } else {
        None
    };
    Ok(emit_numbered_body(
        &header,
        &lines,
        &selected,
        footer.as_deref(),
    ))
}

/// Render a chainable post-edit hashline preview for `new_text`.
///
/// The header carries the post-edit tag. For ordinary edits, body rows use
/// **post-edit** line numbers around `focus_lines` so a follow-up `edit` can
/// copy them without a full re-read. Large unchanged spans collapse to `…`.
///
/// `structural` omits numbered body lines entirely and requires a re-read
/// before further ops on this path — a focused window is not a safe anchor map
/// after a large rewrite.
pub(crate) fn format_post_edit_preview(
    display_path: &str,
    new_text: &str,
    focus_lines: &[usize],
    structural: bool,
) -> String {
    let header = format_header(display_path, &compute_file_hash(new_text));
    if structural {
        let total = split_content_lines(new_text).len();
        return format!(
            "{header}\n\n[{STRUCTURAL_EDIT_FOOTER_MARKER}; {total} line(s) after apply; re-read this path before further edit ops — {STRUCTURAL_NO_CHAIN_MARKER}]"
        );
    }
    let lines = split_content_lines(new_text);
    if lines.is_empty() {
        return header;
    }
    let total = lines.len();
    let mut focus = sanitize_focus(focus_lines, total);
    if focus.is_empty() {
        focus.push(1);
    }
    let expanded = expand_focus_lines(&focus, total, POST_EDIT_CONTEXT_LINES);
    let selected = cap_selected_by_hunk(&expanded, POST_EDIT_MAX_BODY_LINES);
    let footer = post_edit_footer(&selected, total);
    emit_numbered_body(&header, &lines, &selected, footer.as_deref())
}

/// Bounded hashline snapshot for chaining after `write` or a failed `edit`.
///
/// Always includes the full-file TAG. Body rows are capped: focus anchors expand
/// locally; otherwise a short head+tail window is used.
pub(crate) fn format_chain_snapshot(
    display_path: &str,
    text: &str,
    focus_lines: &[usize],
) -> String {
    let lines = split_content_lines(text);
    let header = format_header(display_path, &compute_file_hash(text));
    if lines.is_empty() {
        return header;
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
    emit_numbered_body(&header, &lines, &selected, footer.as_deref())
}

fn post_edit_footer(selected: &[usize], total: usize) -> Option<String> {
    if selected.is_empty() {
        return None;
    }
    let first = selected[0];
    let last = *selected.last().expect("non-empty");
    if first == 1 && last == total && selected.len() == total {
        return None;
    }
    Some(format!(
        "[post-edit lines {first}-{last} of {total} shown around changes; re-read for other lines]"
    ))
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

#[derive(Clone, Copy)]
struct Fnv1a32 {
    hash: u32,
}

impl Fnv1a32 {
    const OFFSET_BASIS: u32 = 0x811c_9dc5;
    const PRIME: u32 = 0x0100_0193;

    const fn new() -> Self {
        Self {
            hash: Self::OFFSET_BASIS,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.hash ^= u32::from(byte);
            self.hash = self.hash.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u32 {
        self.hash
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
