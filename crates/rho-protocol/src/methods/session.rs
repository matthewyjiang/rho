//! Sessions: conversations the daemon hosts, named by their session id, the
//! id `rho sessions` and `/resume` show. While the daemon has a session
//! loaded, its turns are a job (see [`super::JOB_LIST`]); each prompt runs
//! one turn in the daemon, which keeps running when the client that sent it
//! disconnects. A client that attaches to a session receives its events as
//! notifications until it detaches (#1657).

use std::{collections::BTreeMap, fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ContentBlock, Cursor, InputRequest, JobState, Seq, SessionEvent, SessionSetup, StreamId,
};

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
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
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

/// A client's environment variables, which a daemon session's children
/// see in place of the daemon's own (#1659): its tools' commands and
/// background processes (minus provider credentials), its MCP servers,
/// hooks, and OpenAPI sources, its CLI subagents and Claude Code advisor
/// (credentials included), and the workflow runs it starts. Values are
/// secrets: the daemon never logs or saves them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ClientEnvironment(BTreeMap<String, String>);

impl ClientEnvironment {
    /// This process's environment. A variable whose name or value is not
    /// UTF-8 is left out, since the wire carries strings.
    pub fn of_this_process() -> Self {
        std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .collect()
    }

    /// Every variable, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

impl FromIterator<(String, String)> for ClientEnvironment {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(variables: I) -> Self {
        Self(variables.into_iter().collect())
    }
}

impl IntoIterator for ClientEnvironment {
    type Item = (String, String);
    type IntoIter = std::collections::btree_map::IntoIter<String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Shows only how many variables there are: the values are secrets.
impl fmt::Debug for ClientEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientEnvironment")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Params of [`SESSION_NEW`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionNewParams {
    /// The session's workspace, an absolute path to an existing directory.
    /// The daemon reads config and agents for it as `rho` started there
    /// would.
    pub cwd: PathBuf,
    /// The client's environment, which the session's processes run with
    /// until a prompt brings another.
    pub environment: ClientEnvironment,
    /// The agent the session runs as, by id, one of the workspace's as
    /// `rho --agent` finds them; absent, the default agent. One the
    /// workspace does not define, or that cannot run as a session's own
    /// agent, is [`crate::ErrorCode::SessionOpenFailed`]. A session reopened
    /// later runs as the agent it was saved as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The client's choices for the session.
    #[serde(default, skip_serializing_if = "SessionSetup::is_default")]
    pub setup: SessionSetup,
}

/// Result of [`SESSION_NEW`]: the session is loaded and its job is
/// [`crate::JobState::Idle`]. `rho sessions` lists the session once its
/// first prompt is saved; [`super::JOB_LIST`] shows its job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionNewResponse {
    pub session: SessionId,
    /// The session's settings and the choices it offers, which
    /// [`super::SESSION_CONFIGURE`] changes.
    pub options: super::SessionOptions,
}

/// Params of [`SESSION_PROMPT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPromptParams {
    pub session: SessionId,
    /// The user message: at least one block, or the prompt is refused as
    /// [`crate::ErrorCode::InvalidParams`], as is one with an image whose
    /// data is not a base64 PNG, JPEG, GIF, or WebP of the declared type
    /// within the daemon's image limit. Text and images reach the model as
    /// written. A resource reaches it as its text, and a resource link as
    /// its description, else its name, each fenced under its URI (#1670). A
    /// slash command the message starts with that the session runs, such as
    /// `/init`, a prompt template, or an MCP prompt, is expanded first, as
    /// `rho run` expands it.
    pub content: Vec<ContentBlock>,
    /// How long the turn may run, in milliseconds from when the daemon
    /// takes the prompt. A turn still running then is cancelled and ends
    /// [`crate::TurnOutcome::TimedOut`]. `None` sets no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// The prompting client's environment, which the processes this turn
    /// starts run with, and those of later turns until another prompt
    /// brings another. Processes already running keep theirs.
    pub environment: ClientEnvironment,
}

/// Result of [`SESSION_PROMPT`]: the prompt is saved and its turn is
/// running. The turn runs on in the daemon; [`super::JOB_LIST`] reports how
/// it ends.
///
/// A turn that ends before its prompt is saved answers with an error
/// instead: [`crate::ErrorCode::TurnEnded`], whose `data` is the turn's
/// [`crate::TurnOutcome`], or [`crate::ErrorCode::DaemonStopping`] when the
/// daemon stopped it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPromptResponse {}

/// Params of [`SESSION_CANCEL`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCancelParams {
    pub session: SessionId,
}

/// Result of [`SESSION_CANCEL`]: the turn in progress, if any, has been
/// asked to stop. The turn saves what it did and ends
/// [`crate::JobState::Cancelled`], or [`crate::JobState::Interrupted`] when
/// the daemon is stopping; one already past its
/// [`SessionPromptParams::timeout_ms`] ends [`crate::JobState::TimedOut`].
/// A cancel that arrives before a prompted turn begins still cancels it,
/// and that prompt answers [`crate::ErrorCode::TurnEnded`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCancelResponse {}

/// Params of [`SESSION_STEER`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSteerParams {
    pub session: SessionId,
    /// Input for the running turn's next model request.
    pub text: String,
}

/// Result of [`SESSION_STEER`]: the input is queued for the running turn,
/// which applies it at its next model request. A turn that ends first drops
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSteerResponse {}

/// Params of [`SESSION_ATTACH`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionAttachParams {
    pub session: SessionId,
    /// The last event this client has, to resume after it. Absent, from
    /// another stream, or older than the daemon keeps: [`Replay::Reset`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Cursor>,
    /// The environment the session opens with if this attach reopens it
    /// (#1661): its processes run with it until a prompt brings another.
    /// A session the daemon has loaded keeps its own.
    pub environment: ClientEnvironment,
    /// The client's choices for the session if this attach reopens it. A
    /// session the daemon has loaded keeps its own, and one a prompt
    /// reopened has none.
    #[serde(default, skip_serializing_if = "SessionSetup::is_default")]
    pub setup: SessionSetup,
}

/// Result of [`SESSION_ATTACH`]. The session's events then arrive as
/// [`SESSION_EVENT`] notifications, never before this response. Attaching
/// again on the same connection replaces the earlier attachment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionAttachResponse {
    /// The stream the events belong to; a cursor names it to resume.
    pub stream: StreamId,
    /// The directory the session works in: the one it was saved in, for a
    /// session the daemon reopened (#1661).
    pub workspace: PathBuf,
    pub replay: Replay,
    /// How many events the replay holds: the first this many
    /// [`SESSION_EVENT`] notifications after this response replay what the
    /// client missed, and the rest are live (#1669). A reset replays the
    /// last turn's [`super::SessionEvent::TurnEnded`] until the next turn
    /// saves, so a client tells how the turn it attached after ended only
    /// once it knows where the replay stops.
    pub replay_events: u64,
    /// The session's job state when the attachment began, which the replay
    /// ends in.
    pub state: JobState,
    /// The input requests open when the attachment began, oldest first:
    /// what a client may answer even when the replay no longer holds the
    /// events that opened them (#1660). The replay may repeat some; an
    /// input request's id names it.
    pub inputs: Vec<InputRequest>,
    /// The session's title when the attachment began; absent until it has
    /// one. Saved transcripts hold no [`super::SessionEvent::TitleChanged`],
    /// so a reset may not replay the one that set it (#1670).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The session's options when the attachment began; a later
    /// [`super::SessionEvent::OptionsChanged`] replaces them.
    pub options: super::SessionOptions,
}

/// What the events after a [`SessionAttachResponse`] start with.
///
/// Closed on purpose, unlike the event enums: a client cannot follow events
/// it does not know how to start from, so a new kind is a protocol major.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Replay {
    /// The events after the client's cursor follow.
    Tail,
    /// Discard what you have for this session, cursor included: its saved
    /// transcript follows, then the turn in progress.
    Reset,
}

/// Params of [`SESSION_DETACH`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionDetachParams {
    pub session: SessionId,
}

/// Result of [`SESSION_DETACH`]: no events of the session follow. Detaching
/// a session this connection has not attached also succeeds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionDetachResponse {}

/// Params of the [`SESSION_EVENT`] notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionEventParams {
    pub session: SessionId,
    /// Rises by one per live event. A reset's transcript events carry 0,
    /// except the last, which carries the seq the turn in progress began
    /// after. So the highest seq a client has seen since its attach
    /// response, once above 0, is always a cursor it can resume from: one
    /// that detaches partway through a transcript resets again.
    pub seq: Seq,
    pub event: SessionEvent,
}

/// Params of the [`SESSION_DETACHED`] notification. No events of the
/// session follow; attach again, with the client's cursor, to resume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionDetachedParams {
    pub session: SessionId,
    pub reason: DetachReason,
}

/// Why the daemon dropped an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DetachReason {
    /// The client fell too far behind, or the daemon unloaded the session,
    /// as a delete that failed after it unloaded the session does; attach
    /// again with the client's cursor.
    Lagged,
    /// The daemon is stopping.
    DaemonStopping,
    /// The session was deleted ([`super::SESSION_DELETE`]).
    Deleted,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
