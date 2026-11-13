//! A session's background work: what its turns started that runs on its
//! own, which attached clients follow and may stop.

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

/// Params of [`SESSION_BACKGROUND_STOP`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundStopParams {
    pub session: SessionId,
    pub work: BackgroundItem,
}

/// Result of [`SESSION_BACKGROUND_STOP`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBackgroundStopResponse {}

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
