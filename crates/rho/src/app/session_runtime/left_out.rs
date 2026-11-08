//! The parts a session's host left out of it (`--no-tools`,
//! `--no-system-prompt`, `--no-subagents`, or a daemon client's setup),
//! kept in its file so the daemon reopens it without them: a session a
//! client narrowed stays narrowed when the daemon unloads it and a prompt
//! or attach reopens it (`daemon_host::sessions::setup`).
//!
//! Like the added directories (`added_dirs.rs`), the record rides in
//! snapshot metadata and every durable save rewrites it with what the
//! runtime leaves out. A session nothing was ever left out of keeps no
//! record, and one saved without the record, such as before it existed,
//! reads as leaving nothing out.
//!
//! The guarantee is the daemon's: it holds across daemon reopens. The TUI,
//! not a daemon client yet, keeps an existing record but does not record
//! its own flags, until it becomes one (#1667).

use rho_sdk::SessionSnapshot;
use serde::{Deserialize, Serialize};

/// The snapshot metadata key of a session's [`LeftOut`], a JSON object with
/// a flag per part.
const METADATA_KEY: &str = "rho.without";

/// The parts left out of a session. A record naming a part this build does
/// not know fails to decode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LeftOut {
    /// Rho's system prompt, as with `--no-system-prompt`.
    pub(crate) system_prompt: bool,
    /// Every tool, as with `--no-tools`.
    pub(crate) tools: bool,
    /// The tools that start subagents, as with `--no-subagents`.
    pub(crate) subagents: bool,
}

impl LeftOut {
    /// The parts `stored` was last saved without. A record this build
    /// cannot read fails, rather than reopen the session with parts it left
    /// out. Only the daemon, which is unix-only so far (#1665), reopens a
    /// session with what it saved.
    #[cfg(unix)]
    pub(crate) fn saved(stored: &crate::session::Session) -> anyhow::Result<Self> {
        use anyhow::Context;

        let Some(encoded) = stored.session_metadata(METADATA_KEY)? else {
            return Ok(Self::default());
        };
        serde_json::from_str(&encoded)
            .context("could not read the parts the session was saved without")
    }

    /// The parts either leaves out.
    #[cfg(unix)]
    pub(crate) fn union(self, other: Self) -> Self {
        let Self {
            system_prompt,
            tools,
            subagents,
        } = self;
        Self {
            system_prompt: system_prompt || other.system_prompt,
            tools: tools || other.tools,
            subagents: subagents || other.subagents,
        }
    }

    /// `snapshot` with these parts in its metadata. One that leaves nothing
    /// out and had no record keeps none.
    pub(super) fn decorate(self, snapshot: SessionSnapshot) -> SessionSnapshot {
        if self == Self::default() && !snapshot.metadata().contains_key(METADATA_KEY) {
            return snapshot;
        }
        let encoded = serde_json::to_string(&self).expect("part flags encode as JSON");
        snapshot.with_metadata(METADATA_KEY, encoded)
    }
}
