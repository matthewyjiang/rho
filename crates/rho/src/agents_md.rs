//! Markdown assembly for appending a standing instruction to AGENTS.md.

/// Preserve existing instructions, separating the new bullet with one newline.
pub(crate) fn appended_contents(existing: Option<&str>, text: &str) -> String {
    let existing = existing.unwrap_or_default().trim_end_matches(['\r', '\n']);
    let text = text.trim();
    if existing.is_empty() {
        format!("- {text}\n")
    } else {
        format!("{existing}\n- {text}\n")
    }
}

#[cfg(test)]
#[path = "agents_md_tests.rs"]
mod tests;
