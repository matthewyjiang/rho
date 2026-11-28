//! A session's background work: what its turns started that runs on its
//! own, which attached clients follow and may stop, and any client lists
//! for every loaded session at once.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// Stop one piece of a loaded session's background work. Answered once the
/// stop is asked for, not once the work ended: a
/// [`super::SessionEvent::Background`] that no longer lists it as running
/// says that. Work the session does not know, or that already ended, is
/// [`crate::ErrorCode::InvalidParams`]; an unloaded session is
/// [`crate::ErrorCode::SessionNotLoaded`].
pub const SESSION_BACKGROUND_STOP: &str = "rho/session/background/stop";

/// Hold a loaded session's background results for its next turn: until a
/// turn of the session's begins, the daemon starts no turn of its own to
/// give the model the results of work that finished, even for a session
/// it wakes ([`super::IdleWake::Turn`]), and the session's background work,
/// running or ended, keeps it neither loaded, nor an upgrade waiting, nor
/// from settling: an idle unload ends what still runs and drops the
/// results. The next turn takes the results, and ends the hold.
///
/// For a client that stops a session's work as it closes it, such as an
/// ACP v2 host's `session/close`: held before the cancel, the work the
/// cancel and the stops end starts no turn after the close. An unloaded
/// session is [`crate::ErrorCode::SessionNotLoaded`]; a daemon from before
/// this method answers [`crate::ErrorCode::MethodNotFound`].
pub const SESSION_BACKGROUND_HOLD: &str = "rho/session/background/hold";

/// List the background work of every loaded session that has some, as each
/// session's last [`super::SessionEvent::Background`] did, without
/// attaching to any. Work runs on after the turn that started it ends, so
/// a session whose job is no longer in progress may still list some; a
/// client that shows sessions from [`super::JOB_LIST`] reads this to tell
/// one that is done from one whose work still runs. Reissue to refresh. A
/// daemon from before this method answers [`crate::ErrorCode::MethodNotFound`].
///
/// # Next major
///
/// NEXT_MAJOR(rho-protocol): list each session's background work as a `background` field of `JobLive` and remove `rho/session/background/list`.
///
/// `JobLive` is not `#[non_exhaustive]`, so a field is a major change.
pub const SESSION_BACKGROUND_LIST: &str = "rho/session/background/list";

/// Params of [`SESSION_BACKGROUND_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundListParams {}

/// Result of [`SESSION_BACKGROUND_LIST`]: a point-in-time read, not a
/// subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundListResponse {
    /// Each loaded session with background work, by session id. A session
    /// absent here has none, or is not loaded. A subagent's session lists
    /// none while its run is under way; the run is a
    /// [`BackgroundItem::Subagent`] of its parent's.
    pub sessions: Vec<SessionBackground>,
}

/// One loaded session's background work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackground {
    pub session: SessionId,
    /// What still runs, and what ended whose result the session's model has
    /// not been given, as [`super::SessionEvent::Background`] lists it;
    /// never empty. [`BackgroundStatus::is_running`] tells which runs.
    pub work: Vec<BackgroundWork>,
}

/// Params of [`SESSION_BACKGROUND_STOP`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundStopParams {
    pub session: SessionId,
    pub work: BackgroundItem,
}

/// Result of [`SESSION_BACKGROUND_STOP`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundStopResponse {}

/// Params of [`SESSION_BACKGROUND_HOLD`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundHoldParams {
    pub session: SessionId,
}

/// Result of [`SESSION_BACKGROUND_HOLD`]: the session's results wait for
/// its next turn.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundHoldResponse {}

/// One piece of a session's background work, as
/// [`super::SessionEvent::Background`] lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackgroundWork {
    pub item: BackgroundItem,
    /// What it shows as: a process's command line, a workflow's name, a
    /// subagent's title.
    pub title: String,
    pub status: BackgroundStatus,
    /// A process's exit code, once it exited with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// What a piece of background work is, and the id the session names it by.
/// Protocol minors may add kinds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackgroundItem {
    /// A process the session's `process` tool started, by its
    /// [`super::SessionProcess::id`].
    Process { id: String },
    /// A workflow run the session started.
    Workflow { run: String },
    /// A subagent the daemon runs for the session, by its run id.
    Subagent {
        run: String,
        /// The subagent's own session, once it opened one; runtimes that
        /// keep no Rho session never have one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },
    /// A kind a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Where a piece of background work is. Protocol minors may add statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackgroundStatus {
    Running,
    /// It finished by itself: a process exited, with any code; a workflow or
    /// subagent succeeded.
    Completed,
    /// It failed, timed out, or never started.
    Failed,
    /// Someone stopped it.
    Cancelled,
    /// A status a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl BackgroundStatus {
    /// Whether the work still runs. A status this crate does not know
    /// counts as running, so unknown work never looks finished.
    pub fn is_running(self) -> bool {
        match self {
            Self::Completed | Self::Failed | Self::Cancelled => false,
            Self::Running | Self::Unknown => true,
        }
    }
}
