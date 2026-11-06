//! Sessions: conversations the daemon hosts, named by their session id, the
//! id `rho sessions` and `/resume` show. While the daemon has a session
//! loaded, its turns are a job (see [`super::JOB_LIST`]); each prompt runs
//! one turn in the daemon, which keeps running when the client that sent it
//! disconnects. A client that attaches to a session receives its events as
//! notifications until it detaches (#1657).

use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::{Cursor, JobState, Method, Notification, Seq, SessionEvent, StreamId};

/// Open a new session in a workspace, before its first prompt.
pub const SESSION_NEW: &str = "rho/session/new";
/// Start a turn on a session that has none in progress.
pub const SESSION_PROMPT: &str = "rho/session/prompt";
/// Cancel a session's turn in progress, if any.
pub const SESSION_CANCEL: &str = "rho/session/cancel";
/// Stage input for a running turn's next model request.
pub const SESSION_STEER: &str = "rho/session/steer";
/// Receive a session's events as [`SESSION_EVENT`] notifications.
pub const SESSION_ATTACH: &str = "rho/session/attach";
/// Stop receiving a session's events.
pub const SESSION_DETACH: &str = "rho/session/detach";

/// Notification: one event of an attached session.
pub const SESSION_EVENT: &str = "rho/session/event";
/// Notification: the daemon dropped this client's attachment to a session.
pub const SESSION_DETACHED: &str = "rho/session/detached";

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

/// Marker for [`SESSION_ATTACH`].
pub struct SessionAttach;

impl Method for SessionAttach {
    const NAME: &'static str = SESSION_ATTACH;
    type Params = SessionAttachParams;
    type Result = SessionAttachResponse;
}

/// Marker for [`SESSION_DETACH`].
pub struct SessionDetach;

impl Method for SessionDetach {
    const NAME: &'static str = SESSION_DETACH;
    type Params = SessionDetachParams;
    type Result = SessionDetachResponse;
}

/// Marker for the [`SESSION_EVENT`] notification.
pub struct SessionEventNotification;

impl Notification for SessionEventNotification {
    const NAME: &'static str = SESSION_EVENT;
    type Params = SessionEventParams;
}

/// Marker for the [`SESSION_DETACHED`] notification.
pub struct SessionDetachedNotification;

impl Notification for SessionDetachedNotification {
    const NAME: &'static str = SESSION_DETACHED;
    type Params = SessionDetachedParams;
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
/// first prompt is saved; [`super::JOB_LIST`] shows its job.
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
/// running. The turn runs on in the daemon; [`super::JOB_LIST`] reports how
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

/// Params of [`SESSION_ATTACH`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAttachParams {
    pub session: SessionId,
    /// The last event this client has, to resume after it. Absent, from
    /// another stream, or older than the daemon keeps: [`Replay::Reset`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Cursor>,
}

/// Result of [`SESSION_ATTACH`]. The session's events then arrive as
/// [`SESSION_EVENT`] notifications, never before this response. Attaching
/// again on the same connection replaces the earlier attachment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAttachResponse {
    /// The stream the events belong to; a cursor names it to resume.
    pub stream: StreamId,
    pub replay: Replay,
    /// The session's job state when the attachment began.
    pub state: JobState,
}

/// What the events after a [`SessionAttachResponse`] start with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Replay {
    /// The events after the client's cursor follow.
    Tail,
    /// Discard what you have for this session: its saved transcript
    /// follows, then the turn in progress.
    Reset,
}

/// Params of [`SESSION_DETACH`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetachParams {
    pub session: SessionId,
}

/// Result of [`SESSION_DETACH`]: no events of the session follow. Detaching
/// a session this connection has not attached also succeeds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetachResponse {}

/// Params of the [`SESSION_EVENT`] notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventParams {
    pub session: SessionId,
    /// Rises by one per live event. Replayed transcript events carry the
    /// seq the turn in progress started after, so the highest seq a client
    /// has seen is always its cursor.
    pub seq: Seq,
    pub event: SessionEvent,
}

/// Params of the [`SESSION_DETACHED`] notification. No events of the
/// session follow; attach again, with the client's cursor, to resume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetachedParams {
    pub session: SessionId,
    pub reason: DetachReason,
}

/// Why the daemon dropped an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DetachReason {
    /// The client fell too far behind; reattach with its cursor.
    Lagged,
    /// The daemon is stopping.
    DaemonStopping,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    Unknown,
}
