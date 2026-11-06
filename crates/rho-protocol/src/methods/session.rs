//! Sessions: conversations the daemon hosts, named by their session id, the
//! id `rho sessions` and `/resume` show. While the daemon has a session
//! loaded, its turns are a job (see [`super::JOBS_LIST`]); each prompt runs
//! one turn in the daemon, which keeps running when the client that sent it
//! disconnects.

use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::Method;

/// Open a new session in a workspace, before its first prompt.
pub const SESSION_NEW: &str = "rho/session/new";
/// Start a turn on a session that has none in progress.
pub const SESSION_PROMPT: &str = "rho/session/prompt";
/// Cancel a session's turn in progress, if any.
pub const SESSION_CANCEL: &str = "rho/session/cancel";
/// Stage input for a running turn's next model request.
pub const SESSION_STEER: &str = "rho/session/steer";

/// A session's id, the one `rho sessions` and `/resume` show. Opaque to
/// clients: send back the string the daemon gave.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Marker for [`SESSION_NEW`].
pub struct SessionNew;

impl Method for SessionNew {
    const NAME: &'static str = SESSION_NEW;
    type Params = SessionNewParams;
    type Result = SessionNewResponse;
}

/// Marker for [`SESSION_PROMPT`].
pub struct SessionPrompt;

impl Method for SessionPrompt {
    const NAME: &'static str = SESSION_PROMPT;
    type Params = SessionPromptParams;
    type Result = SessionPromptResponse;
}

/// Marker for [`SESSION_CANCEL`].
pub struct SessionCancel;

impl Method for SessionCancel {
    const NAME: &'static str = SESSION_CANCEL;
    type Params = SessionCancelParams;
    type Result = SessionCancelResponse;
}

/// Marker for [`SESSION_STEER`].
pub struct SessionSteer;

impl Method for SessionSteer {
    const NAME: &'static str = SESSION_STEER;
    type Params = SessionSteerParams;
    type Result = SessionSteerResponse;
}

/// Params of [`SESSION_NEW`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionNewParams {
    /// The session's workspace, an absolute path to an existing directory.
    /// The daemon reads config and agents for it as `rho` started there
    /// would.
    pub cwd: PathBuf,
}

/// Result of [`SESSION_NEW`]: the session is loaded and its job is
/// [`crate::JobState::Idle`]. `rho sessions` lists the session once its
/// first prompt is saved; [`super::JOBS_LIST`] shows its job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionNewResponse {
    pub session: SessionId,
}

/// Params of [`SESSION_PROMPT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPromptParams {
    pub session: SessionId,
    /// The user message, sent to the model as written.
    pub text: String,
}

/// Result of [`SESSION_PROMPT`]: the prompt is saved and its turn is
/// running. The turn runs on in the daemon; [`super::JOBS_LIST`] reports how
/// it ends.
///
/// A turn that ends before its prompt is saved answers with an error
/// instead: [`crate::ErrorCode::TurnFailed`] with the reason,
/// [`crate::ErrorCode::TurnCancelled`] when a cancel stopped it, or
/// [`crate::ErrorCode::DaemonStopping`] when the daemon stopped it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPromptResponse {}

/// Params of [`SESSION_CANCEL`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCancelParams {
    pub session: SessionId,
}

/// Result of [`SESSION_CANCEL`]: the turn in progress, if any, has been
/// asked to stop. The turn saves what it did and ends
/// [`crate::JobState::Cancelled`], or [`crate::JobState::Interrupted`] when
/// the daemon is stopping. A cancel that arrives before a prompted turn
/// begins still cancels it, and that prompt answers
/// [`crate::ErrorCode::TurnCancelled`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCancelResponse {}

/// Params of [`SESSION_STEER`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSteerParams {
    pub session: SessionId,
    /// Input for the running turn's next model request.
    pub text: String,
}

/// Result of [`SESSION_STEER`]: the input is queued for the running turn,
/// which applies it at its next model request. A turn that ends first drops
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSteerResponse {}
