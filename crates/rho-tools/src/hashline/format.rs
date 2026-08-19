//! Hashline snapshot tags and tagged views.
//!
//! Tags are harness-local fingerprints of normalized file text. They are not a
//! global content-addressed store; any reader that sees the same bytes mints the
//! same tag, and editors reject a tag when the live file no longer matches.
//!
//! Numbered rows live in [`crate::text_view`]. This module only wraps those
//! views with `[path#TAG]`.

use crate::text_view::{
    cap_selected_by_hunk, emit_numbered_body, expand_focus_lines,
    format_chain_snapshot as format_numbered_snapshot, format_numbered_view, split_content_lines,
    LineFingerprint,
};

/// Number of uppercase hex digits in a file snapshot tag.
///
/// Four hex digits (low 16 bits of FNV-1a) match the oh-my-pi wire format and
/// keep headers cheap. The tag is a freshness lock: after the live file drifts,
/// `edit` fails closed and returns a new live snapshot to copy.
pub(crate) const FILE_HASH_LENGTH: usize = 4;

/// Separator between a path and its snapshot tag inside a section header.
pub(crate) const FILE_HASH_SEP: char = '#';

/// Compute the snapshot tag for normalized file text.
pub(crate) fn compute_file_hash(text: &str) -> String {
    compute_file_hash_bytes(text.as_bytes())
}

/// Byte-equivalent of [`compute_file_hash`] for streaming readers.
///
/// Hashing is ASCII-whitespace-insensitive on each `\n` segment, so it matches
/// the UTF-8 string path without decoding the whole file.
pub(crate) fn compute_file_hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = FileHash::new();
    for line in bytes.split(|&byte| byte == b'\n') {
        hasher.push_line(line);
    }
    hasher.finish()
}

fn trim_hash_line(line: &[u8]) -> &[u8] {
    let end = line
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
        .map_or(0, |index| index + 1);
    &line[..end]
}

fn format_file_hash(digest: u32) -> String {
    format!("{:04X}", digest & 0xFFFF)
}

/// Streaming FNV-1a fingerprint of `\n` segments, matching [`compute_file_hash`].
pub(crate) struct FileHash {
    hasher: Fnv1a32,
    started: bool,
}

impl FileHash {
    pub(crate) fn new() -> Self {
        Self {
            hasher: Fnv1a32::new(),
            started: false,
        }
    }
}

impl LineFingerprint for FileHash {
    fn push_line(&mut self, line: &[u8]) {
        if self.started {
            self.hasher.write(b"\n");
        }
        self.started = true;
        self.hasher.write(trim_hash_line(line));
    }

    fn finish(self) -> String {
        format_file_hash(self.hasher.finish())
    }
}

/// Format a section header `[path#TAG]`.
pub(crate) fn format_header(path: &str, file_hash: &str) -> String {
    format!("[{path}{FILE_HASH_SEP}{file_hash}]")
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

/// Marker text embedded in structural post-edit footers (tests assert against this).
pub(crate) const STRUCTURAL_EDIT_FOOTER_MARKER: &str = "structural edit";

/// Marker that structural previews omit numbered chainable body lines.
pub(crate) const STRUCTURAL_NO_CHAIN_MARKER: &str = "no chainable body lines";

/// Render a hashline view of `text` for `display_path`.
///
/// `offset`/`limit` select a 1-indexed inclusive window of lines. The header
/// carries the full-file tag so later hashline edits can validate the snapshot.
pub(crate) fn format_hashline_view(
    display_path: &str,
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    format_numbered_view(
        &format_header(display_path, &compute_file_hash(text)),
        text,
        offset,
        limit,
    )
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
    let mut focus: Vec<usize> = focus_lines
        .iter()
        .copied()
        .filter(|line| *line >= 1 && *line <= total)
        .collect();
    focus.sort_unstable();
    focus.dedup();
    if focus.is_empty() {
        focus.push(1);
    }
    let expanded = expand_focus_lines(&focus, total, POST_EDIT_CONTEXT_LINES);
    let selected = cap_selected_by_hunk(&expanded, POST_EDIT_MAX_BODY_LINES);
    let footer = post_edit_footer(&selected, total);
    emit_numbered_body(&header, &lines, &selected, footer.as_deref())
}

/// Bounded hashline snapshot after `write` or a failed `edit`.
pub(crate) fn format_chain_snapshot(
    display_path: &str,
    text: &str,
    focus_lines: &[usize],
) -> String {
    format_numbered_snapshot(
        &format_header(display_path, &compute_file_hash(text)),
        text,
        focus_lines,
    )
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

    fn finish(self) -> u32 {
        self.hash
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
