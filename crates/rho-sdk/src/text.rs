//! Shared UTF-8 byte-index helpers and truncation markers.
//!
//! [`floor_char_boundary`] and [`ceil_char_boundary`] mirror the standard
//! library methods stabilized in Rust 1.91. The crate MSRV is older, so callers
//! use these helpers instead of hand-rolling the walk. Drop them when the MSRV
//! reaches 1.91.
//!
//! Workspace crates and out-of-tree callers that import these root exports must
//! depend on a published `rho-sdk` version that includes them. Bump `rho-sdk`
//! (and any re-exporting crate) together when this public surface changes.

/// Marker appended when model-facing or tool output is cut at a byte budget.
///
/// Callers that must keep a hard byte budget subtract this before choosing the
/// cut they pass, so the marker cannot push the result past their limit.
/// Downstream crates that import this constant need a published `rho-sdk`
/// version that exports it.
pub const TRUNCATION_MARKER: &str = "\n[truncated]";

/// Marker appended when a provider diagnostic is cut at its display budget.
///
/// Downstream crates that import this constant need a published `rho-sdk`
/// version that exports it.
pub const DIAGNOSTIC_TRUNCATION_MARKER: &str = "\n[diagnostic truncated]";

/// ASCII ellipsis used when a short inline preview is clipped.
pub const ASCII_ELLIPSIS: &str = "...";

/// Unicode ellipsis used when a UI preview keeps a single-character marker.
pub const ELLIPSIS: &str = "…";

/// Floors `index` to the previous UTF-8 character boundary.
///
/// Indices past the end of `value` clamp to `value.len()`. Index `0` is always
/// a boundary.
pub fn floor_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Ceils `index` to the next UTF-8 character boundary.
///
/// Indices past the end of `value` clamp to `value.len()`.
pub fn ceil_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
