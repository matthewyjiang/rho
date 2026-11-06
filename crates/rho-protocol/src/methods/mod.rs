//! Method and notification names and their params and result types.
//!
//! Names are `rho/<area>/<verb>`, except the connection-level `rho/hello`.
//! Params and results ignore unknown fields so a protocol minor can add
//! optional fields without breaking older peers. Ids are strings.
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | [`HELLO`] | [`HelloParams`] | [`HelloResponse`] |
//! | [`DAEMON_STATUS`] | [`StatusParams`] | [`DaemonStatus`] |
//! | [`DAEMON_STOP`] | [`StopParams`] | [`StopResponse`] |
//! | [`SESSION_NEW`] | [`SessionNewParams`] | [`SessionNewResponse`] |
//! | [`SESSION_PROMPT`] | [`SessionPromptParams`] | [`SessionPromptResponse`] |
//! | [`SESSION_CANCEL`] | [`SessionCancelParams`] | [`SessionCancelResponse`] |
//! | [`SESSION_STEER`] | [`SessionSteerParams`] | [`SessionSteerResponse`] |
//! | [`SESSION_ATTACH`] | [`SessionAttachParams`] | [`SessionAttachResponse`] |
//! | [`SESSION_DETACH`] | [`SessionDetachParams`] | [`SessionDetachResponse`] |
//! | [`JOB_LIST`] | [`JobListParams`] | [`JobListResponse`] |
//!
//! The daemon sends these notifications; clients send none.
//!
//! | Notification | Params |
//! |---|---|
//! | [`SESSION_EVENT`] | [`SessionEventParams`] |
//! | [`SESSION_DETACHED`] | [`SessionDetachedParams`] |

use serde::{de::DeserializeOwned, Serialize};

mod daemon;
mod event;
mod job;
mod session;

pub use daemon::{
    ClientInfo, DaemonInfo, DaemonStatus, Hello, HelloParams, HelloResponse, Status, StatusParams,
    Stop, StopParams, StopResponse, DAEMON_STATUS, DAEMON_STOP, HELLO,
};
pub use event::{
    ContentBlock, Cursor, Location, MessageId, PlanEntry, PlanStatus, Seq, SessionEvent, StreamId,
    ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
pub use job::{
    JobId, JobKind, JobList, JobListParams, JobListResponse, JobState, JobSummary, JOB_LIST,
};
pub use session::{
    DetachReason, Replay, SessionAttach, SessionAttachParams, SessionAttachResponse, SessionCancel,
    SessionCancelParams, SessionCancelResponse, SessionDetach, SessionDetachParams,
    SessionDetachResponse, SessionDetachedNotification, SessionDetachedParams,
    SessionEventNotification, SessionEventParams, SessionId, SessionNew, SessionNewParams,
    SessionNewResponse, SessionPrompt, SessionPromptParams, SessionPromptResponse, SessionSteer,
    SessionSteerParams, SessionSteerResponse, SESSION_ATTACH, SESSION_CANCEL, SESSION_DETACH,
    SESSION_DETACHED, SESSION_EVENT, SESSION_NEW, SESSION_PROMPT, SESSION_STEER,
};

/// A request method with typed params and result.
///
/// Implemented by zero-sized markers so [`crate::Client::request`] can check
/// a call's types at compile time. Implementors pair `NAME` with the params
/// and result types the daemon uses for that method.
pub trait Method {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned;
    type Result: Serialize + DeserializeOwned;
}

/// A notification the daemon sends, typed like [`Method`].
///
/// Implemented by zero-sized markers so the daemon encodes, and
/// [`crate::Client`] decodes, each notification with one params type.
pub trait Notification {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned;
}
