//! Cursor effort is a trailing model-id suffix (`-low`, `-high`, `-xhigh`), not a
//! request field. Product names keep those words in the middle of the id.

use crate::reasoning::ReasoningLevel;

/// Effort to encode on one Cursor Run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CursorEffort {
    /// Leave a baked suffix in place, or omit effort when the catalog id has none.
    #[default]
    Unspecified,
    /// Replace any baked suffix with this level when it has a Cursor token.
    Level(ReasoningLevel),
}

/// Split a trailing Cursor effort suffix from a catalog or wire id.
pub(crate) fn split_effort(model: &str) -> (&str, Option<ReasoningLevel>) {
    // Longer tokens first so `-xhigh` is not parsed as `-high`.
    const SUFFIXES: &[(&str, ReasoningLevel)] = &[
        ("-xhigh", ReasoningLevel::Xhigh),
        ("-minimal", ReasoningLevel::Minimal),
        ("-medium", ReasoningLevel::Medium),
        ("-high", ReasoningLevel::High),
        ("-low", ReasoningLevel::Low),
        ("-max", ReasoningLevel::Max),
    ];
    for (suffix, level) in SUFFIXES {
        if let Some(base) = model.strip_suffix(suffix).filter(|base| !base.is_empty()) {
            return (base, Some(*level));
        }
    }
    (model, None)
}

/// Strip effort words Cursor puts on display names for suffixed variants.
pub(crate) fn strip_effort_display_suffix(name: &str) -> &str {
    const SUFFIXES: &[&str] = &[
        " Extra High",
        " XHigh",
        " xhigh",
        " Minimal",
        " Medium",
        " High",
        " Low",
        " Max",
    ];
    for suffix in SUFFIXES {
        if let Some(base) = name.strip_suffix(suffix).filter(|base| !base.is_empty()) {
            return base;
        }
    }
    name
}

#[cfg(test)]
#[path = "effort_tests.rs"]
mod tests;
