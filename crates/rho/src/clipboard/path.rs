use std::path::{Path, PathBuf};

/// Resolves a single-line paste that names a supported regular file.
pub(crate) fn paste_text_as_file_path(text: &str, cwd: &Path) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return None;
    }
    let (unquoted, _was_quoted) = strip_matching_quotes(trimmed);
    if unquoted.is_empty() {
        return None;
    }
    if unquoted.contains("://") {
        if let Ok(url) = url::Url::parse(unquoted) {
            if url.scheme() == "file" {
                return supported_file_path(url.to_file_path().ok()?, cwd);
            }
            return None;
        }
    }
    let candidate = PathBuf::from(unquoted);
    if let Some(path) = supported_file_path(candidate, cwd) {
        return Some(path);
    }
    #[cfg(unix)]
    if !_was_quoted && unquoted.contains('\\') {
        return supported_file_path(PathBuf::from(unescape_shell_path(unquoted)), cwd);
    }
    None
}

fn supported_file_path(candidate: PathBuf, cwd: &Path) -> Option<PathBuf> {
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
    (path.is_file() && path_has_supported_attachment_extension(&path)).then_some(path)
}

#[cfg(unix)]
fn unescape_shell_path(path: &str) -> String {
    let mut unescaped = String::with_capacity(path.len());
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            if let Some(escaped) = characters.next() {
                unescaped.push(escaped);
            } else {
                unescaped.push(character);
            }
        } else {
            unescaped.push(character);
        }
    }
    unescaped
}

fn path_has_supported_attachment_extension(path: &Path) -> bool {
    if rho_tools::document::path_has_supported_document_extension(path) {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif")
    )
}

fn strip_matching_quotes(text: &str) -> (&str, bool) {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return (&text[1..text.len() - 1], true);
        }
    }
    (text, false)
}
