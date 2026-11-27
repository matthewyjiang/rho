//! Reloading a loaded session's hooks from their files, the one way a
//! client changes them without opening the session again.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// Reload a loaded session's hooks from their files, between its turns, as
/// rho's TUI does for `/hooks`. Reading the session's hooks report
/// ([`super::SessionReport::Hooks`]) never does.
///
/// A file that cannot be read or validated leaves the session's hooks as
/// they were, and so does a kind of hook (blocking or observing) the session
/// started without, which loads only when the session is opened again: the
/// response's [`HooksReloadOutcome::Refused`] says why. An unloaded session
/// is [`crate::ErrorCode::SessionNotLoaded`], and one in a turn
/// [`crate::ErrorCode::SessionBusy`]. A daemon that predates it answers
/// [`crate::ErrorCode::MethodNotFound`].
pub const SESSION_HOOKS_RELOAD: &str = "rho/session/hooks/reload";

/// Params of [`SESSION_HOOKS_RELOAD`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionHooksReloadParams {
    pub session: SessionId,
}

impl SessionHooksReloadParams {
    /// Builds it from each of its fields.
    pub fn new(session: SessionId) -> Self {
        Self { session }
    }
}

/// Result of [`SESSION_HOOKS_RELOAD`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionHooksReloadResponse {
    /// Whether the session now runs what its hooks files define, or why not.
    pub outcome: HooksReloadOutcome,
}

impl SessionHooksReloadResponse {
    /// Builds it from each of its fields.
    pub fn new(outcome: HooksReloadOutcome) -> Self {
        Self { outcome }
    }
}

/// What a [`SESSION_HOOKS_RELOAD`] did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HooksReloadOutcome {
    /// The session now runs what its hooks files define.
    Reloaded,
    /// The session keeps the hooks it ran: a hooks file does not load, the
    /// files add a kind of hook it started without, or a hook started it, so
    /// it runs none. `reason` says which, as rho words it.
    Refused { reason: String },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
