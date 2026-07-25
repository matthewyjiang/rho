//! Blocking persistence for Claude CLI session artifacts.
//!
//! Stream effects enqueue without awaits. A dedicated OS thread owns
//! `result.json` and attachment JSONL writes. Rate-limit observations use a
//! coalescing slot outside the bounded queue so transcript pressure cannot
//! drop them. The terminal barrier flushes prior work, acks the final status,
//! and only then publishes the single terminal watch update.

mod sink;
mod worker;

#[cfg(test)]
pub(crate) use sink::{EmergencyWriteHooks, PersistShutdownBudgets};
pub(crate) use sink::{SinkConfig, StatusSink};
#[cfg(test)]
pub(crate) use worker::{AttachmentKind, PersistLogEntry, WriterStall};
pub(crate) use worker::{ClaudeRunIdentity, PersistHooks};

#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;
