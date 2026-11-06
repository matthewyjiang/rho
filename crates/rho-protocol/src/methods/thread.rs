//! Threads: conversations the daemon hosts. A thread is a job (see
//! [`super::JOBS_LIST`]) with one session; each prompt runs one turn in the
//! daemon, which keeps running when the client that sent it disconnects.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{JobId, Method};

/// Open a new thread in a workspace, before its first prompt.
pub const THREAD_CREATE: &str = "rho/thread/create";
/// Start a turn on a thread that has none in progress.
pub const THREAD_PROMPT: &str = "rho/thread/prompt";
/// Cancel a thread's turn in progress, if any.
pub const THREAD_CANCEL: &str = "rho/thread/cancel";
/// Stage input for a running turn's next model request.
pub const THREAD_STEER: &str = "rho/thread/steer";

/// Marker for [`THREAD_CREATE`].
pub struct ThreadCreate;

impl Method for ThreadCreate {
    const NAME: &'static str = THREAD_CREATE;
    type Params = ThreadCreateParams;
    type Result = ThreadCreateResponse;
}

/// Marker for [`THREAD_PROMPT`].
pub struct ThreadPrompt;

impl Method for ThreadPrompt {
    const NAME: &'static str = THREAD_PROMPT;
    type Params = ThreadPromptParams;
    type Result = ThreadPromptResponse;
}

/// Marker for [`THREAD_CANCEL`].
pub struct ThreadCancel;

impl Method for ThreadCancel {
    const NAME: &'static str = THREAD_CANCEL;
    type Params = ThreadCancelParams;
    type Result = ThreadCancelResponse;
}

/// Marker for [`THREAD_STEER`].
pub struct ThreadSteer;

impl Method for ThreadSteer {
    const NAME: &'static str = THREAD_STEER;
    type Params = ThreadSteerParams;
    type Result = ThreadSteerResponse;
}

/// Params of [`THREAD_CREATE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCreateParams {
    /// The thread's workspace, an absolute path to an existing directory.
    /// The daemon reads config and agents for it as a session opened there
    /// would.
    pub cwd: PathBuf,
}

/// Result of [`THREAD_CREATE`]: the thread is open and
/// [`crate::JobState::Idle`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCreateResponse {
    pub thread: JobId,
    /// The id of the thread's session, which `rho sessions` lists once its
    /// first prompt is saved.
    pub session_id: String,
}

/// Params of [`THREAD_PROMPT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadPromptParams {
    pub thread: JobId,
    /// The user message, sent to the model as written.
    pub text: String,
}

/// Result of [`THREAD_PROMPT`]: the turn has started and its prompt is
/// saved. It runs on in the daemon; [`super::JOBS_LIST`] reports how it
/// ends.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadPromptResponse {}

/// Params of [`THREAD_CANCEL`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCancelParams {
    pub thread: JobId,
}

/// Result of [`THREAD_CANCEL`]: the turn in progress, if any, has been
/// asked to stop. It saves what it did and ends
/// [`crate::JobState::Cancelled`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCancelResponse {}

/// Params of [`THREAD_STEER`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSteerParams {
    pub thread: JobId,
    /// Input for the running turn's next model request.
    pub text: String,
}

/// Result of [`THREAD_STEER`]: the input is queued for the running turn,
/// which applies it at its next model request. A turn that ends first drops
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSteerResponse {}
