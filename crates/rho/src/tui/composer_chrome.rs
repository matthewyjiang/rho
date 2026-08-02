//! Shared selection markers and footer separators for composer modes.
//!
//! Keep marker and separator vocabulary here so pickers, approvals, inline
//! choice, questionnaires, and text inputs do not drift.

/// Active row marker for single-choice lists.
pub(super) const SELECTION_MARKER_ACTIVE: &str = "→";
/// Inactive row marker for single-choice lists.
pub(super) const SELECTION_MARKER_INACTIVE: &str = " ";
/// Separator between key-hint segments in composer footers.
pub(super) const FOOTER_SEPARATOR: &str = " · ";

/// Join non-empty footer segments with [`FOOTER_SEPARATOR`].
pub(super) fn join_footer_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(FOOTER_SEPARATOR)
}
