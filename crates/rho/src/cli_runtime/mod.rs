//! Process infrastructure for external CLI agents.
//!
//! Nothing here knows which tool is being run: process supervision, executable
//! resolution, Windows shim argument encoding, and bounded stderr capture are
//! the same regardless of the CLI on the other end. Tool-specific policy
//! (which program, how to map a missing binary, what the args mean) stays with
//! the runtime that owns the tool, such as [`crate::claude_runtime`].

mod child;
mod executable;
mod stderr_tail;
mod windows_shim_args;

pub(crate) use child::OwnedChild;
pub(crate) use executable::{CliExecutable, CliExecutableError};
pub(crate) use stderr_tail::StderrTail;
#[cfg(test)]
pub(crate) use stderr_tail::MAX_STDERR_BYTES;
