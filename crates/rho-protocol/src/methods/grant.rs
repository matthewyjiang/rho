//! What a session's turn may do to other sessions (#1780).

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the turn a request starts may do through session control, the
/// `sessions` tool and `rho mcp serve`: how deep the session sits among the
/// sessions that started it, and the most it may do to others.
///
/// The request that opens or prompts a session names it
/// ([`super::SessionNewParams::grant`], [`super::SessionPromptParams::grant`]),
/// and the daemon keeps it on the session's job
/// ([`super::JobSummary::grant`]), so it is the daemon's record, not the
/// caller's environment, that says what a session may do. The daemon sets
/// `RHO_SESSION_DEPTH` and `RHO_SESSION_SCOPE` in the session's environment
/// from it, replacing any the client's environment carries.
///
/// It bounds session control, not the session: the depth stops a runaway
/// recursion and the scope keeps a caller's limits on the sessions it
/// starts. Neither is a sandbox for a session that can run commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionGrant {
    /// How many sessions started one another to reach this one: 0 for a
    /// session a person drives, 1 for one that session's agent started.
    pub depth: u32,
    pub access: GrantAccess,
    pub workspaces: GrantWorkspaces,
}

/// What a grant lets its holder do to sessions. Both sides read it, and it
/// is closed: an access a daemon does not know must fail, never widen.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum GrantAccess {
    /// List, read, and wait for sessions.
    Read,
    /// Also start, prompt, steer, and cancel them.
    Write,
}

/// The workspaces a grant reaches. Closed, as [`GrantAccess`] is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantWorkspaces {
    Any,
    /// These directories and everything under them, as canonical paths.
    Only(Vec<PathBuf>),
}
