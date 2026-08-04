//! Hashline display format: file snapshot tags and numbered line views.
//!
//! Tags are harness-local fingerprints of normalized file text. They are not a
//! global content-addressed store; any reader that sees the same bytes mints the
//! same tag, and editors reject a tag when the live file no longer matches.

/// Number of uppercase hex digits in a file snapshot tag.
pub const FILE_HASH_LENGTH: usize = 4;

/// Separator between a path and its snapshot tag inside a section header.
pub const FILE_HASH_SEP: char = '#';

/// Separator between a 1-indexed line number and the line body.
pub const LINE_BODY_SEP: char = ':';

/// Compute the 4-hex snapshot tag for normalized file text.
pub fn compute_file_hash(text: &str) -> String {
    let normalized = normalize_for_hash(text);
    let digest = fnv1a32(normalized.as_bytes()) & 0xffff;
    format!("{digest:04X}")
}

/// Format a section header `[path#TAG]`.
pub fn format_header(path: &str, file_hash: &str) -> String {
    format!("[{path}{FILE_HASH_SEP}{file_hash}]")
}

/// Format one numbered content line as `N:text` (no trailing newline).
pub fn format_numbered_line(line_number: usize, line: &str) -> String {
    format!("{line_number}{LINE_BODY_SEP}{line}")
}

/// Render a hashline view of `text` for `display_path`.
///
/// `offset`/`limit` select a 1-indexed inclusive window of lines. The header
/// always carries the full-file tag so later edits can validate the snapshot.
pub fn format_hashline_view(
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
    if total == 0 {
        if start > 1 {
            return Err(format!(
                "offset {start} is past the end of the file (0 line(s))"
            ));
        }
        let hash = compute_file_hash(text);
        return Ok(format_header(display_path, &hash));
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
    let hash = compute_file_hash(text);
    let mut out = format_header(display_path, &hash);
    out.push('\n');
    for line_number in start..=end {
        out.push_str(&format_numbered_line(line_number, lines[line_number - 1]));
        out.push('\n');
    }
    // Drop the trailing newline so truncation and tool output stay tidy, matching
    // ordinary file bodies that the model already expects without a final blank.
    out.pop();
    if limit.is_some() && end < total {
        out.push_str(&format!(
            "\n\n[{end} of {total} lines shown; re-read with a higher limit or later offset for the rest]"
        ));
    }
    Ok(out)
}

/// Split file text into addressable content lines.
///
/// A trailing newline does not create an extra blank line. An empty file has
/// zero lines. A file whose sole content is a blank line (single `\n`) has one
/// empty line.
pub fn split_content_lines(text: &str) -> Vec<&str> {
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
pub fn detect_eol(text: &str) -> &'static str {
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
pub fn has_trailing_newline(text: &str) -> bool {
    text.ends_with('\n') || text.ends_with('\r')
}

/// Strip trailing spaces/tabs/CR from each line before hashing so display trim
/// and CRLF endings do not invalidate a tag.
pub fn normalize_for_hash(text: &str) -> String {
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
