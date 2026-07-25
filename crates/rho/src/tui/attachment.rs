//! Read-only observation of persisted subagent presentation events.

mod app;
mod journal;
mod sdk_writer;

pub(crate) use app::run;
pub(crate) use journal::{AttachmentEvent, AttachmentWriter};
pub(crate) use sdk_writer::SdkAttachmentWriter;
