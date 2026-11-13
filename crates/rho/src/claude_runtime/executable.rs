//! Locate `claude-agent-acp`, the ACP adapter Rho runs Claude Code through.
//!
//! The adapter bundles the Claude CLI it drives and runs that CLI with the
//! arguments after [`CLI`]. Sign-in, auth status, the version probe, and the
//! `/usage` panel all go through it, so one install serves every Claude
//! surface and the CLI always matches the adapter's Agent SDK. Resolution and
//! invocation rules are generic and live in [`crate::cli_runtime`].

use crate::{cli_runtime::CliExecutable, session_environment::Ambient};

use super::auth::ClaudeAuthError;

/// Program name resolved on the `PATH` of the environment it runs in.
pub(crate) const ADAPTER_PROGRAM: &str = "claude-agent-acp";

/// `claude-agent-acp --cli <args>` runs the bundled Claude CLI with `<args>`.
pub(crate) const CLI: &str = "--cli";

/// How to install the adapter, for missing-binary errors.
pub(crate) const INSTALL_HINT: &str =
    "install it with `npm install -g @agentclientprotocol/claude-agent-acp` (or `bun add -g`)";

/// Locate `claude-agent-acp` on the `PATH` of `environment`, the one it will
/// run in. On Windows, prefer real binaries and then `.cmd` / `.ps1` shims
/// that Rust's bare-name lookup will not find.
pub(crate) fn resolve(environment: &Ambient) -> Result<CliExecutable, ClaudeAuthError> {
    CliExecutable::resolve_in(ADAPTER_PROGRAM, environment.path().as_deref())
        .ok_or(ClaudeAuthError::BinaryMissing)
}
