//! Shared selection markers and footer separators for composer modes.
//!
//! Keep marker and separator vocabulary here so pickers, approvals, inline
//! choice, questionnaires, and text inputs do not drift.

use unicode_width::UnicodeWidthStr;

/// Active row marker for single-choice lists.
pub(super) const SELECTION_MARKER_ACTIVE: &str = "→";
/// Inactive row marker for single-choice lists.
pub(super) const SELECTION_MARKER_INACTIVE: &str = " ";
/// Separator between key-hint segments in composer footers.
pub(super) const FOOTER_SEPARATOR: &str = " · ";

/// Which composer rule may carry captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComposerDividerSlot {
    /// Above the composer. May show shell mode and advisor captions.
    Top,
    /// Below the composer. Rule only.
    Bottom,
}

/// Join non-empty footer segments with [`FOOTER_SEPARATOR`].
pub(super) fn join_footer_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(FOOTER_SEPARATOR)
}

/// Pack footer segments onto as few lines as possible without splitting one.
///
/// A segment wider than `width` stays on its own line so the caller can
/// truncate it as a last resort. Empty parts are skipped.
pub(super) fn wrap_footer_parts<'a>(
    parts: impl IntoIterator<Item = &'a str>,
    width: usize,
) -> Vec<String> {
    let width = width.max(1);
    let sep_width = UnicodeWidthStr::width(FOOTER_SEPARATOR);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for part in parts.into_iter().filter(|part| !part.is_empty()) {
        let part_width = UnicodeWidthStr::width(part);
        if current.is_empty() {
            current.push_str(part);
            current_width = part_width;
            continue;
        }
        if current_width
            .saturating_add(sep_width)
            .saturating_add(part_width)
            <= width
        {
            current.push_str(FOOTER_SEPARATOR);
            current.push_str(part);
            current_width = current_width
                .saturating_add(sep_width)
                .saturating_add(part_width);
            continue;
        }
        lines.push(std::mem::take(&mut current));
        current.push_str(part);
        current_width = part_width;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
#[path = "composer_chrome_tests.rs"]
mod tests;
