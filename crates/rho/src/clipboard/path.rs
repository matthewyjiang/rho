use std::path::{Path, PathBuf};

/// Resolves a single-line pasted path to an existing regular file.
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
    let path = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };
    path.canonicalize().ok().filter(|path| path.is_file())
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
