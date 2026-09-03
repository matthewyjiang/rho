//! Cursor Agent program names and sink labels.

#![allow(dead_code)] // Phase D pickers / login

use crate::claude_runtime::persist::RuntimeLabel;

/// Program name resolved on `PATH`. Not `agent`: that collides with other tools.
pub(crate) const CURSOR_PROGRAM: &str = "cursor-agent";

/// How Cursor names itself in a `<source>/<model>` slot.
///
/// Phase D wires this into pickers and `/login cursor`.
pub(crate) const CURSOR_SOURCE_LABEL: &str = "cursor";

/// Error prefixes and [`RuntimeLabel::program`]: `cursor: ...`.
pub(crate) const CURSOR_PROGRAM_LABEL: &str = "cursor";

/// Starting activity and program name for the shared artifact sink.
pub(crate) const CURSOR_LABEL: RuntimeLabel = RuntimeLabel {
    starting_activity: "starting cursor",
    program: CURSOR_PROGRAM_LABEL,
};
