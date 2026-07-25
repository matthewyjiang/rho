//! Claude run artifact adapter.
//!
//! Persistence mechanics live in [`crate::run_artifacts`]. This module only
//! translates Claude stream effects onto that shared contract.

mod sink;

#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;

pub(crate) use sink::{ClaudeRunIdentity, StatusSink};
