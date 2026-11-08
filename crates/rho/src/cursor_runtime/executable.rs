//! Locate the `cursor-agent` program.
//!
//! Resolution and invocation rules are generic and live in
//! [`crate::cli_runtime`]; the only Cursor-specific policy is how a missing
//! binary is reported. Claude has no path-env override, so neither does this.

use crate::{cli_runtime::CliExecutable, session_environment::Ambient};

use super::{auth::CursorAuthError, models::CURSOR_PROGRAM};

/// Locate `cursor-agent` on the `PATH` of `environment`, the one it will run
/// in. On Windows, prefer real binaries and then `.cmd` / `.ps1` shims that
/// Rust's bare-name lookup will not find.
pub(crate) fn resolve(environment: &Ambient) -> Result<CliExecutable, CursorAuthError> {
    CliExecutable::resolve_in(CURSOR_PROGRAM, environment.path().as_deref())
        .ok_or(CursorAuthError::BinaryMissing)
}
