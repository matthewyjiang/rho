//! Generic external CLI subagent runtime infrastructure.
//!
//! Provides process supervision, Windows shim argument encoding, executable
//! resolution, and bounded stderr tail capture shared across external CLI
//! agent runtimes (such as Claude Code or future Cursor CLI).

pub(crate) mod child;
pub(crate) mod executable;
pub(crate) mod stderr_tail;
pub(crate) mod windows_shim_args;

pub(crate) use child::OwnedChild;
pub(crate) use executable::{resolve_named, CliExecutable, CliExecutableError};
pub(crate) use stderr_tail::StderrTail;

#[cfg(test)]
pub(crate) use executable::{CliArgv, CliInvocationKind};
#[cfg(test)]
pub(crate) use windows_shim_args::{bat_command_line, WindowsShimArgError};
