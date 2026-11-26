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
/// response says the reload did not apply and the hooks report says why. An
/// unloaded session is [`crate::ErrorCode::SessionNotLoaded`], and one with a
/// turn or a change under way [`crate::ErrorCode::SessionBusy`]. A daemon
/// that predates it answers [`crate::ErrorCode::MethodNotFound`].
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
    /// Whether the session now runs what its hooks files define. False when
    /// they could not load in full, so it keeps the hooks it ran; its hooks
    /// report says why.
    pub reloaded: bool,
}

impl SessionHooksReloadResponse {
    /// Builds it from each of its fields.
    pub fn new(reloaded: bool) -> Self {
        Self { reloaded }
    }
}
