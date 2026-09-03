//! Resolve the `claude` program and build fixed-argv process commands.
//!
//! Re-exports and delegates to [`crate::cli_runtime::executable`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::cli_runtime::{resolve_named as cli_resolve_named, CliExecutable, CliExecutableError};
#[cfg(test)]
use crate::cli_runtime::{CliArgv, CliInvocationKind};

use super::auth::{ClaudeAuthError, CLAUDE_PROGRAM};

#[cfg(test)]
pub(crate) type ClaudeInvocationKind = CliInvocationKind;
pub(crate) type ClaudeExecutableError = CliExecutableError;
#[cfg(test)]
pub(crate) type ClaudeArgv = CliArgv;

/// Resolved path and invocation strategy for Claude Code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeExecutable(CliExecutable);

impl ClaudeExecutable {
    pub(crate) fn from_path(path: impl Into<PathBuf>) -> Self {
        Self(CliExecutable::from_path(path))
    }

    pub(crate) fn from_cli(cli: CliExecutable) -> Self {
        Self(cli)
    }

    pub(crate) fn display(&self) -> String {
        self.0.display()
    }

    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    #[cfg(test)]
    pub(crate) fn program(&self) -> &Path {
        self.0.path()
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> ClaudeInvocationKind {
        self.0.kind()
    }

    #[cfg(test)]
    pub(crate) fn plan<I, S>(&self, args: I) -> Result<ClaudeArgv, ClaudeExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.0.plan(args)
    }

    pub(crate) fn try_command<I, S>(&self, args: I) -> Result<Command, ClaudeExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.0.try_command(args)
    }
}

/// Locate `claude` for spawning. On Windows, prefer real binaries and then
/// `.cmd` / `.ps1` shims that Rust's bare-name lookup will not find.
pub(crate) fn resolve() -> Result<ClaudeExecutable, ClaudeAuthError> {
    resolve_named(CLAUDE_PROGRAM)
}

pub(crate) fn resolve_named(program: &str) -> Result<ClaudeExecutable, ClaudeAuthError> {
    cli_resolve_named(program)
        .map(ClaudeExecutable::from_cli)
        .ok_or(ClaudeAuthError::BinaryMissing)
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
