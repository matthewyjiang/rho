//! Method names and their params and result types.
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
//! | [`JOBS_LIST`] | [`JobsListParams`] | [`JobsListResponse`] |

use serde::{de::DeserializeOwned, Serialize};

mod daemon;
mod jobs;
mod session;

pub use daemon::{
    ClientInfo, DaemonInfo, DaemonStatus, Hello, HelloParams, HelloResponse, Status, StatusParams,
    Stop, StopParams, StopResponse, DAEMON_STATUS, DAEMON_STOP, HELLO,
};
pub use jobs::{
    JobId, JobKind, JobState, JobSummary, JobsList, JobsListParams, JobsListResponse, JOBS_LIST,
};
pub use session::{
    SessionCancel, SessionCancelParams, SessionCancelResponse, SessionId, SessionNew,
    SessionNewParams, SessionNewResponse, SessionPrompt, SessionPromptParams,
    SessionPromptResponse, SessionSteer, SessionSteerParams, SessionSteerResponse, SESSION_CANCEL,
    SESSION_NEW, SESSION_PROMPT, SESSION_STEER,
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
