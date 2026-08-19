//! Read-only observation of persisted subagent presentation events.
//!
//! Journal types live in [`crate::run_artifacts`]. This module owns the attach
//! TUI and the Rho SDK event translator.

mod app;
mod chrome;
pub(crate) mod sdk_writer;
mod select;
mod tool_toggle;

pub(crate) use app::{run, AttachInput, AttachmentApp, AttachmentDisplaySettings};
pub(crate) use chrome::embedded_footer_hint;
pub(crate) use sdk_writer::translate_run_event;
