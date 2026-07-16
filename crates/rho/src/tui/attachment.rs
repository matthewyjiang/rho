//! Read-only observation of persisted subagent presentation events.

mod app;
mod journal;

pub(crate) use app::run;
pub(crate) use journal::AttachmentWriter;
