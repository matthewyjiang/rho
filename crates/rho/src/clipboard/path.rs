use std::path::{Path, PathBuf};

/// Parses a single-line pasted path without accessing the filesystem.
pub(crate) fn paste_text_as_file_path(text: &str, cwd: &Path) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return None;
    }
    let unquoted = strip_matching_quotes(trimmed);
    if unquoted.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(unquoted);
    if !candidate.is_absolute()
        && candidate.extension().is_none()
        && candidate.components().count() == 1
    {
        return None;
    }
    let path = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };
    Some(path)
}

fn strip_matching_quotes(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &text[1..text.len() - 1];
        }
    }
    text
}
