//! Process infrastructure for external CLI agents.
//!
//! Nothing here knows which tool is being run: process supervision, executable
//! resolution, Windows shim argument encoding, bounded probes, bounded
//! stderr capture, frozen identity overlay, log tails, NDJSON line bounds,
//! status sink, stream-effect vocabulary, and payload formatting are the same
//! regardless of the agent on the other end. Tool-specific policy (which
//! program, how to map a missing binary, what the session options mean, how
//! to persist rate limits) stays with the runtime that owns the tool (Claude
//! Code, Cursor, …).

pub(crate) mod agent_event;
mod child;
mod executable;
mod frozen_args;
pub(crate) mod line_decoder;
mod log_tail;
mod overrides;
pub(crate) mod parent_messages;
mod probe;
pub(crate) mod status_sink;
mod stderr_tail;
pub(crate) mod stream_effect;
pub(crate) mod stream_format;
mod windows_shim_args;

pub(crate) use child::OwnedChild;
pub(crate) use executable::{CliExecutable, CliExecutableError};
pub(crate) use frozen_args::overlay_identity_flags;
pub(crate) use log_tail::read_log_tail;
pub(crate) use overrides::{BeforeSpawn, CliSessionOverrides};
#[cfg(all(test, unix))]
pub(crate) use probe::{run_bounded_command_with_timeout, PROBE_OUTPUT_CAP_BYTES};
pub(crate) use probe::{run_bounded_probe, BoundedOutput, ProbeError};
pub(crate) use stderr_tail::StderrTail;
#[cfg(all(test, unix))]
pub(crate) use stderr_tail::MAX_STDERR_BYTES;
