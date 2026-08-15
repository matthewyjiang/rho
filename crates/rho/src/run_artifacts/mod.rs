//! Shared run-artifact contract for delegated agents.
//!
//! Owns `result.json` status writes and the `events.jsonl` attachment journal.
//! Runtime adapters (Rho SDK, Claude stream-json) translate into this surface;
//! the TUI only reads and renders it.

mod journal;
mod sink;

#[cfg(test)]
pub(crate) use journal::AttachmentWriter;
pub(crate) use journal::{AttachmentEvent, AttachmentReader};
pub(crate) use sink::{LiveRunTitle, RunArtifactIdentity, RunArtifactSink};
