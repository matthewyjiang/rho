//! Process infrastructure for external CLI agents.
//!
//! Nothing here knows which tool is being run: process supervision, executable
//! resolution, Windows shim argument encoding, bounded probes, bounded
//! stderr capture, frozen identity overlay, and log tails are the same
//! regardless of the CLI on the other end. Tool-specific policy (which
//! program, how to map a missing binary, what the args mean) stays with the
//! runtime that owns the tool, such as [`crate::claude_runtime`].

mod child;
mod executable;
mod frozen_args;
mod log_tail;
mod probe;
mod stderr_tail;
mod windows_shim_args;

pub(crate) use child::OwnedChild;
pub(crate) use executable::{CliExecutable, CliExecutableError};
pub(crate) use frozen_args::overlay_identity_flags;
pub(crate) use log_tail::read_log_tail;
#[cfg(test)]
pub(crate) use probe::{run_bounded_command_with_timeout, PROBE_OUTPUT_CAP_BYTES};
pub(crate) use probe::{run_bounded_probe, BoundedOutput, ProbeError};
pub(crate) use stderr_tail::StderrTail;
#[cfg(test)]
pub(crate) use stderr_tail::MAX_STDERR_BYTES;
