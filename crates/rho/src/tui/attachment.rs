//! Read-only observation of persisted subagent presentation events.
//!
//! Journal types live in [`crate::run_artifacts`]. This module owns the attach
//! TUI and the Rho SDK event translator.

mod app;
pub(crate) mod sdk_writer;
mod tool_toggle;

pub(crate) use app::{run, AttachmentDisplaySettings};
pub(crate) use sdk_writer::translate_run_event;
