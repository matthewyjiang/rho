//! The parts a session's host left out of it (`--no-tools`,
//! `--no-system-prompt`, `--no-subagents`, or a daemon client's setup),
//! kept in its file so the daemon reopens it without them: a session a
//! client narrowed stays narrowed when the daemon unloads it and a prompt
//! or attach reopens it (`daemon_host::sessions::setup`).
//!
//! Like the added directories (`added_dirs.rs`), the record rides in
//! snapshot metadata and every durable save rewrites it with what the
//! runtime leaves out. A session nothing was ever left out of keeps no
//! record, and one saved without the record, such as by the TUI or before
//! it existed, reads as leaving nothing out.

use rho_sdk::SessionSnapshot;
use serde::{Deserialize, Serialize};

use super::SessionLaunch;
use crate::app::agent_binding::LaunchTools;

/// The snapshot metadata key of a session's [`LeftOut`], a JSON array of
/// part names.
const METADATA_KEY: &str = "rho.without";

/// The parts left out of a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LeftOut {
    /// Rho's system prompt, as with `--no-system-prompt`.
    pub(crate) system_prompt: bool,
    /// Every tool, as with `--no-tools`.
    pub(crate) tools: bool,
    /// The tools that start subagents, as with `--no-subagents`.
    pub(crate) subagents: bool,
}

/// A part as the record names it: the daemon protocol's name for it.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Part {
    SystemPrompt,
    Tools,
    Subagents,
}

impl LeftOut {
    /// What `launch` leaves out.
    pub(super) fn of(launch: &SessionLaunch) -> Self {
        Self {
            system_prompt: launch.no_system_prompt,
            tools: launch.no_tools,
            subagents: launch.no_subagents,
        }
    }

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
        let parts: Vec<Part> = serde_json::from_str(&encoded)
            .context("could not read the parts the session was saved without")?;
        let mut left_out = Self::default();
        for part in parts {
            match part {
                Part::SystemPrompt => left_out.system_prompt = true,
                Part::Tools => left_out.tools = true,
                Part::Subagents => left_out.subagents = true,
            }
        }
        Ok(left_out)
    }

    /// The built-in tools a root session without these parts keeps.
    pub(crate) fn launch_tools(self) -> LaunchTools {
        match self {
            Self { tools: true, .. } => LaunchTools::None,
            Self {
                subagents: true, ..
            } => LaunchTools::NoSubagents,
            Self {
                system_prompt: _,
                tools: false,
                subagents: false,
            } => LaunchTools::All,
        }
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
        let Self {
            system_prompt,
            tools,
            subagents,
        } = self;
        let parts: Vec<Part> = [
            (Part::SystemPrompt, system_prompt),
            (Part::Tools, tools),
            (Part::Subagents, subagents),
        ]
        .into_iter()
        .filter_map(|(part, left_out)| left_out.then_some(part))
        .collect();
        if parts.is_empty() && !snapshot.metadata().contains_key(METADATA_KEY) {
            return snapshot;
        }
        let encoded = serde_json::to_string(&parts).expect("part names encode as JSON");
        snapshot.with_metadata(METADATA_KEY, encoded)
    }
}
