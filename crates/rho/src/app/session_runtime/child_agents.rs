//! Who runs the children a session's `agent` tool starts: this process, or
//! the session's host as its own threads (T3 Code's, which `rho acp
//! --t3code` asks the daemon for; see
//! `tools::agent::install_host_delegation`).
//!
//! Like the parts left out (`left_out.rs`), host threads ride in snapshot
//! metadata, so the daemon reopens a T3 thread's session with its children
//! still on host threads. A session never moves its children back into
//! this process: when a prompt, which brings no host MCP servers, reopens
//! it, its `agent` calls fail rather than start a subagent here. A session
//! whose children always ran here keeps no record.

use rho_sdk::SessionSnapshot;
use serde::{Deserialize, Serialize};

/// The snapshot metadata key of a session's [`ChildAgents`], a JSON string.
const METADATA_KEY: &str = "rho.child_agents";

/// Who runs the children the `agent` tool starts. A record naming a value
/// this build does not know fails to decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChildAgents {
    /// Subagents run in this process.
    InProcess,
    /// The host runs them as its own threads, so its UI owns them. The
    /// agent nodes of the session's workflow runs still run in this process.
    HostThreads,
}

impl ChildAgents {
    /// Who runs the children of `stored` as it was last saved. A record
    /// this build cannot read fails, rather than reopen a host-threads
    /// session with its children here. Only the daemon, which is unix-only
    /// so far (#1665), reopens a session with what it saved.
    #[cfg(unix)]
    pub(crate) fn saved(stored: &crate::session::Session) -> anyhow::Result<Self> {
        use anyhow::Context;

        let Some(encoded) = stored.session_metadata(METADATA_KEY)? else {
            return Ok(Self::InProcess);
        };
        serde_json::from_str(&encoded).context("could not read who runs the session's children")
    }

    /// Host threads when either runs its children there.
    #[cfg(unix)]
    pub(crate) fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::InProcess, Self::InProcess) => Self::InProcess,
            (Self::HostThreads, _) | (_, Self::HostThreads) => Self::HostThreads,
        }
    }

    /// `snapshot` with this record in its metadata. In-process children
    /// with no record keep none.
    pub(super) fn decorate(self, snapshot: SessionSnapshot) -> SessionSnapshot {
        if self == Self::InProcess && !snapshot.metadata().contains_key(METADATA_KEY) {
            return snapshot;
        }
        let encoded = serde_json::to_string(&self).expect("a unit variant encodes as JSON");
        snapshot.with_metadata(METADATA_KEY, encoded)
    }
}
