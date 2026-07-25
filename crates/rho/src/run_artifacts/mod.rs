//! Shared run-artifact contract for delegated agents.
//!
//! Owns `result.json` status writes and the `events.jsonl` attachment journal.
//! Runtime adapters (Rho SDK, Claude stream-json) translate into this surface;
//! the TUI only reads and renders it.

mod journal;
mod sink;

pub(crate) use journal::{AttachmentEvent, AttachmentReader, AttachmentWriter};
pub(crate) use sink::{RunArtifactIdentity, RunArtifactSink};
