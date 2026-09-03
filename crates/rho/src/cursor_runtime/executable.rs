//! Locate the `cursor-agent` program.
//!
//! Resolution and invocation rules are generic and live in
//! [`crate::cli_runtime`]; the only Cursor-specific policy is how a missing
//! binary is reported. Claude has no path-env override, so neither does this.

use crate::cli_runtime::CliExecutable;

use super::{auth::CursorAuthError, models::CURSOR_PROGRAM};

/// Locate `cursor-agent` for spawning. On Windows, prefer real binaries and
/// then `.cmd` / `.ps1` shims that Rust's bare-name lookup will not find.
pub(crate) fn resolve() -> Result<CliExecutable, CursorAuthError> {
    CliExecutable::resolve(CURSOR_PROGRAM).ok_or(CursorAuthError::BinaryMissing)
}
