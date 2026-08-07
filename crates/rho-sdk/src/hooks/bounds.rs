use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::floor_char_boundary;

/// Longest string any single payload field may carry before truncation.
pub const DEFAULT_MAX_FIELD_BYTES: usize = 8 * 1024;
/// Largest serialized envelope the runtime will hand to a handler.
pub const DEFAULT_MAX_ENVELOPE_BYTES: usize = 64 * 1024;

/// Explicit size limits applied while building an envelope.
///
/// Bounds are part of the contract, not an optimization: a handler reads one
/// bounded JSON document, so a large tool argument or a long command line can
/// never turn into unbounded child-process input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookPayloadBounds {
    max_field_bytes: usize,
    max_envelope_bytes: usize,
}

impl Default for HookPayloadBounds {
    fn default() -> Self {
        Self {
            max_field_bytes: DEFAULT_MAX_FIELD_BYTES,
            max_envelope_bytes: DEFAULT_MAX_ENVELOPE_BYTES,
        }
    }
}

impl HookPayloadBounds {
    /// Builds bounds, clamping each limit to at least one byte.
    pub fn new(max_field_bytes: usize, max_envelope_bytes: usize) -> Self {
        Self {
            max_field_bytes: max_field_bytes.max(1),
            max_envelope_bytes: max_envelope_bytes.max(1),
        }
    }

    pub fn max_field_bytes(self) -> usize {
        self.max_field_bytes
    }

    pub fn max_envelope_bytes(self) -> usize {
        self.max_envelope_bytes
    }
}

/// What the runtime shortened or removed while building one envelope.
///
/// An empty report means the handler received complete values. Handlers that
/// make decisions on payload text must check this before trusting a match.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HookTruncation {
    truncated: bool,
    /// Dotted field paths that were shortened, for example
    /// `payload.capability.shell_command`.
    fields: BTreeSet<String>,
}

impl HookTruncation {
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn fields(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(String::as_str)
    }

    pub(super) fn record(&mut self, field: impl Into<String>) {
        self.truncated = true;
        self.fields.insert(field.into());
    }
}

/// Shortens `value` to `bounds.max_field_bytes()` on a character boundary.
///
/// Returns whether anything was removed so the caller can name the field in the
/// envelope's truncation report.
pub(super) fn truncate_field(value: &mut String, bounds: HookPayloadBounds) -> bool {
    let limit = bounds.max_field_bytes();
    if value.len() <= limit {
        return false;
    }
    value.truncate(floor_char_boundary(value, limit));
    true
}

pub(super) fn bounded_string(
    value: impl Into<String>,
    field: &str,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> String {
    let mut value = value.into();
    if truncate_field(&mut value, bounds) {
        truncation.record(field);
    }
    value
}

pub(super) fn bounded_path(
    value: &Path,
    field: &str,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> PathBuf {
    let mut rendered = value.to_string_lossy().into_owned();
    let was_lossy = value.to_str().is_none();
    if was_lossy || truncate_field(&mut rendered, bounds) {
        truncation.record(field);
    }
    PathBuf::from(rendered)
}

#[cfg(test)]
#[path = "bounds_tests.rs"]
mod tests;
