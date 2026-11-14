//! Sessions: conversations the daemon hosts, named by their session id, the
//! id `rho sessions` and `/resume` show. While the daemon has a session
//! loaded, its turns are a job (see [`super::JOB_LIST`]); each prompt runs
//! one turn in the daemon, which keeps running when the client that sent it
//! disconnects. A client that attaches to a session receives its events as
//! notifications until it detaches (#1657).

use std::{collections::BTreeMap, ffi::OsString, fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ContentBlock, Cursor, IdempotencyKey, InputRequest, JobState, Seq, SessionEvent, SessionSetup,
    SteerId, StreamId, TurnId,
};

/// Open a new session in a workspace, before its first prompt.
pub const SESSION_NEW: &str = "rho/session/new";
/// Start a turn on a session that has none in progress.
pub const SESSION_PROMPT: &str = "rho/session/prompt";
/// Cancel a session's turn in progress, if any.
pub const SESSION_CANCEL: &str = "rho/session/cancel";
/// Stage input for a running turn's next model request.
pub const SESSION_STEER: &str = "rho/session/steer";
/// Withdraw a steer the running turn has not applied yet.
pub const SESSION_UNSTEER: &str = "rho/session/unsteer";
/// Receive a session's events as [`SESSION_EVENT`] notifications.
pub const SESSION_ATTACH: &str = "rho/session/attach";
/// Receive a session's events as [`SESSION_ATTACH`] does, without loading
/// it: for a client that only reads it.
pub const SESSION_OBSERVE: &str = "rho/session/observe";
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
///
/// Its `RHO_TRUST_PROJECT_*` variables are the session's project trust
/// when it opens or reopens the session: the daemon's own environment
/// grants none. A later prompt's do not change it.
///
/// Its credential variables, such as `OPENAI_API_KEY`, and its
/// `RHO_CREDENTIAL_STORE` are the session's too, from when it opens or
/// reopens until the daemon unloads it: the session's models sign in with
/// them, and a variable in the daemon's own environment never does (#1779).
/// Without the variables the session reads the daemon's own credential
/// store.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ClientEnvironment(BTreeMap<String, String>);

impl ClientEnvironment {
    /// This process's environment. A variable whose name or value is not
    /// UTF-8 is left out, since the wire carries strings. So is one whose
    /// name holds `=`, which no command can be started with: Windows keeps
    /// each drive's working directory in the environment under such a name
    /// (`=C:`), and the daemon refuses an environment that has one.
    pub fn of_this_process() -> Self {
        Self::of(std::env::vars_os())
    }

    /// The variables of `environment` a client sends (see
    /// [`Self::of_this_process`]).
    fn of(environment: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        environment
            .into_iter()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .filter(|(name, _)| !name.contains('='))
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
    /// The session whose turn starts this one, such as the session of an
    /// agent that starts a peer through session control (#1714). The daemon
    /// records the job writing that session as the new session's job's
    /// [`super::JobSummary::parent`], and the new session's turns then wait
    /// for capacity in the daemon-wide agent budget, as subagent runs do
    /// ([`super::JobState::Queued`]). A session no job of the daemon writes,
    /// such as one an in-process rho runs, records no parent, but the new
    /// session's turns still share the budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// What the session may do through session control until a prompt
    /// brings another grant; absent, what `environment` carries
    /// (`RHO_SESSION_DEPTH`, `RHO_SESSION_SCOPE`), which is no limit when
    /// it carries none. With a `parent` whose session a job of the daemon
    /// writes, the session opens with no more than a session that job's
    /// turn starts may have: at least one level deeper than its grant,
    /// with no access or workspace it lacks. A session that opens with a
    /// limit keeps it as its job's [`super::JobSummary::grant_limit`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<super::SessionGrant>,
    /// Whether the session works in a git worktree of its own (#1673), so
    /// sessions in one repository cannot overwrite each other's edits. The
    /// daemon adds a worktree of the repository `cwd` is in, on a new
    /// branch from that repository's `HEAD`, under its rho home, and opens
    /// the session at `cwd`'s place in it. A `cwd` in no git repository,
    /// or one git cannot add a worktree of, is
    /// [`crate::ErrorCode::SessionOpenFailed`]. The daemon never removes
    /// the worktree; [`SessionNewResponse::worktree`] and
    /// [`super::JobSummary::worktree`] name it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worktree: bool,
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
    /// The worktree the session works in, for one opened with
    /// [`SessionNewParams::worktree`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<SessionWorktree>,
}

/// A git worktree the daemon added for a session to work in (#1673).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionWorktree {
    /// The branch checked out in the worktree, new from the repository's
    /// `HEAD` when the session opened.
    pub branch: String,
    /// The worktree's root directory, under the daemon's rho home: what
    /// `git worktree remove` takes.
    pub path: PathBuf,
    /// The workspace the session was asked to open in, in the checkout the
    /// worktree was added from. The session works at the same place in the
    /// worktree.
    pub origin: PathBuf,
}

/// Params of [`SESSION_PROMPT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPromptParams {
    pub session: SessionId,
    /// The user message: at least one block besides
    /// [`ContentBlock::Context`], which reaches the model but is not part of
    /// the saved message, or the prompt is refused as
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
    /// brings another. Processes already running keep theirs. One whose
    /// project trust is not the session's is refused as
    /// [`crate::ErrorCode::ProjectTrustMismatch`].
    pub environment: ClientEnvironment,
    /// What this turn, and later turns until another prompt, may do
    /// through session control; absent, what `environment` carries, as for
    /// [`SessionNewParams::grant`]. The daemon records it on the session's
    /// job when the turn starts, held to the job's
    /// [`super::JobSummary::grant_limit`]: a prompt that asks for more than
    /// the limit is not refused, its turn runs under the limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<super::SessionGrant>,
    /// The client's name for this prompt, which makes sending it again
    /// safe, as after a connection that closed before the answer came. A
    /// prompt whose key is that of the session's latest turn starts none:
    /// it answers as the first one does, with that turn's id once the turn
    /// began, even after the turn ended or the daemon restarted. The daemon
    /// compares only the key, never the content. It keeps the latest turn's
    /// key alone, so a prompt is safe to send again until another turn
    /// starts on the session. A turn that ended before it began ran
    /// nothing and frees its key: sending the prompt again starts a turn.
    /// An empty key is [`crate::ErrorCode::InvalidParams`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
    /// Who the prompt speaks for; absent, the person.
    #[serde(default, skip_serializing_if = "PromptAuthor::is_person")]
    pub author: PromptAuthor,
}

/// Who a prompt speaks for, which decides whether the daemon keeps it as a
/// user message. Protocol minors may add authors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PromptAuthor {
    /// The person: the daemon saves the prompt as their message and shows
    /// it to attached clients.
    #[default]
    Person,
    /// The client, sending a command on the person's behalf, as a workflow
    /// hub's start is: the daemon runs it without saving or showing a user
    /// message for it. Only a `/workflow:` start may come from the client;
    /// any other prompt ends before it began, as a command the session
    /// cannot run does.
    Client,
    /// An author a newer client sends that this daemon does not know; the
    /// prompt is refused as [`crate::ErrorCode::InvalidParams`]. Clients
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl PromptAuthor {
    fn is_person(&self) -> bool {
        *self == Self::Person
    }
}

/// The `data` of a [`crate::ErrorCode::ProjectTrustMismatch`] error: the
/// `RHO_TRUST_PROJECT_*` variables the session's trust and the prompting
/// client's differ in. At least one list has a name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectTrustMismatch {
    /// The variables that grant trust the session opened with and the
    /// prompting client's environment does not, such as
    /// `RHO_TRUST_PROJECT_HOOKS`.
    pub session_only: Vec<String>,
    /// The variables that grant trust the prompting client's environment
    /// does and the session opened without.
    pub client_only: Vec<String>,
}

/// Result of [`SESSION_PROMPT`]: the turn began: it saved its prompt, or,
/// for `/compact`, its compaction started; or, for a session another job
/// started ([`SessionNewParams::parent`]), it queued for capacity in the
/// daemon-wide agent budget. The turn runs on in the daemon;
/// [`super::JOB_LIST`] reports how it ends.
///
/// A turn that ends before it began answers with an error
/// instead: [`crate::ErrorCode::TurnEnded`], whose `data` is the turn's
/// [`crate::TurnOutcome`], or [`crate::ErrorCode::DaemonStopping`] when the
/// daemon stopped it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPromptResponse {
    /// The turn the prompt started, or, for a prompt sent again with its
    /// [`SessionPromptParams::idempotency_key`], the turn it started the
    /// first time. The turn's [`SessionEvent::State`]s and its
    /// [`SessionEvent::TurnEnded`] carry it.
    pub turn: TurnId,
}

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

/// Result of [`SESSION_STEER`]: the running turn accepted the input, which
/// applies at the turn's next model request as a
/// [`SessionEvent::UserMessage`] carrying `id`. A turn that ends first drops
/// it: a client that never saw it apply can offer it again.
///
/// No model request taking input is [`crate::ErrorCode::NotSteerable`], as
/// is a turn that ended before it accepted the input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSteerResponse {
    pub id: SteerId,
}

/// Params of [`SESSION_UNSTEER`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionUnsteerParams {
    pub session: SessionId,
    /// The steer to withdraw, as [`SESSION_STEER`] answered it.
    pub id: SteerId,
}

/// Result of [`SESSION_UNSTEER`]. With no model request taking input, it
/// answers [`crate::ErrorCode::NotSteerable`] instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionUnsteerResponse {
    pub outcome: UnsteerOutcome,
}

/// What [`SESSION_UNSTEER`] did with the steer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnsteerOutcome {
    /// Removed before it reached the conversation: it never applies.
    Withdrawn,
    /// The provider already took it ([`SessionEvent::SteeringDelivered`]):
    /// it still applies.
    Delivered,
    /// Already in the conversation.
    Applied,
    /// The running turn never accepted that id: another turn's, or one a
    /// turn dropped when it ended.
    NotFound,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

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

/// Params of [`SESSION_OBSERVE`]. A session the daemon has loaded is
/// attached as [`SESSION_ATTACH`] attaches it. A saved session it has not
/// loaded stays unloaded: the daemon replays it as its file keeps it, with
/// how its last turn ended, then detaches the client as
/// [`DetachReason::NotLoaded`]. Observing starts nothing of the session, no
/// MCP server, OpenAPI source, or hook, and so carries no environment or
/// setup. With no MCP server connected, such a replay titles the session's
/// MCP tool calls by their names alone.
///
/// A session a subagent run under way is writing is followed through the
/// run's stream, once the run has opened the session; until then the
/// observe is refused as `SessionBusy`. The stream's one turn is the run,
/// and the client is detached as [`DetachReason::NotLoaded`] after the run
/// ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionObserveParams {
    pub session: SessionId,
    /// The last event this client has, to resume after it, as for
    /// [`SESSION_ATTACH`]. A session observed unloaded always resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Cursor>,
}

/// Result of [`SESSION_ATTACH`] and [`SESSION_OBSERVE`]. The session's
/// events then arrive as [`SESSION_EVENT`] notifications, never before this
/// response. Attaching or observing again on the same connection replaces
/// the earlier attachment.
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
    /// saves, under that turn's id.
    pub replay_events: u64,
    /// The session's job state when the attachment began, which the replay
    /// ends in.
    pub state: JobState,
    /// The turn `state` is of: the one under way, or the last one, whose
    /// end a reset's replay holds as the [`super::SessionEvent::TurnEnded`]
    /// carrying this id. Absent only while the session has run no turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnId>,
    /// How long, in milliseconds, `turn` had been under way when the
    /// attachment began, so a client shows the turn's clock from its start
    /// rather than from the attach. Absent while no turn is under way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_elapsed_ms: Option<u64>,
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
    /// The session's background work when the attachment began; a later
    /// [`super::SessionEvent::Background`] replaces it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<super::BackgroundWork>,
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
    /// The client fell too far behind, the daemon unloaded the session, as
    /// a delete that failed after it unloaded the session does, or the
    /// session's conversation moved to another node of its tree; attach
    /// again with the client's cursor.
    Lagged,
    /// The daemon is stopping.
    DaemonStopping,
    /// The daemon is re-executing in place, for an upgrade or a restart, and
    /// closes the connection next. Connect again, which waits until the
    /// daemon answers, and attach with the client's cursor; the restarted
    /// daemon answers with [`Replay::Reset`].
    Restarting,
    /// The session was deleted ([`super::SESSION_DELETE`]).
    Deleted,
    /// An observe ([`SESSION_OBSERVE`]) found the session unloaded: the
    /// replay was the session as its file keeps it, and nothing follows.
    /// Observe again to read it again.
    NotLoaded,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
