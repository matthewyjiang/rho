//! Shared run-artifact contract for delegated agents.
//!
//! Owns `result.json` status writes, the `events.jsonl` attachment journal, and
//! replaceable `tool-previews.json` live argument cards.
//! Runtime adapters (Rho SDK, Claude stream-json) translate into this surface;
//! the TUI only reads and renders it.

mod journal;
mod previews;
mod sink;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
pub(crate) use journal::AttachmentWriter;
pub(crate) use journal::{read_prompt, AttachmentEvent, AttachmentReader};
pub(crate) use sink::{LiveRunTitle, RunArtifactIdentity, RunArtifactSink};
