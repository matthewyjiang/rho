//! Locate the `claude` program.
//!
//! Resolution and invocation rules are generic and live in
//! [`crate::cli_runtime`]; the only Claude-specific policy is how a missing
//! binary is reported.

use crate::cli_runtime::CliExecutable;

use super::auth::{ClaudeAuthError, CLAUDE_PROGRAM};

/// Locate `claude` for spawning. On Windows, prefer real binaries and then
/// `.cmd` / `.ps1` shims that Rust's bare-name lookup will not find.
pub(crate) fn resolve() -> Result<CliExecutable, ClaudeAuthError> {
    resolve_named(CLAUDE_PROGRAM)
}

pub(crate) fn resolve_named(program: &str) -> Result<CliExecutable, ClaudeAuthError> {
    CliExecutable::resolve(program).ok_or(ClaudeAuthError::BinaryMissing)
}
