//! The background processes a loaded session's `process` tool started.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// Read a loaded session's background processes without waiting for its
/// active turn, and one process's retained output when asked. Does not
/// load a session, and reading neither marks a process's result seen nor
/// moves the cursor the session's model polls with. An unloaded session
/// returns [`crate::ErrorCode::SessionNotLoaded`]. Reissue to refresh.
pub const SESSION_PROCESSES: &str = "rho/session/processes";

/// Params of [`SESSION_PROCESSES`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionProcessesParams {
    /// The loaded session whose processes to read.
    pub session: SessionId,
    /// The process whose retained output to read too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ProcessOutputParams>,
}

/// Which of a process's retained output [`SESSION_PROCESSES`] reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessOutputParams {
    /// The process, by its [`SessionProcess::id`].
    pub process: String,
    /// Read the chunks from this cursor on: 0 for everything retained, or
    /// the last answer's [`ProcessOutput::next_cursor`] for what is new.
    pub from: u64,
}

/// Result of [`SESSION_PROCESSES`]: a point-in-time read, not a
/// subscription.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionProcessesResponse {
    /// The processes still running, and those that ended whose result the
    /// session's model has not been given yet, oldest first.
    pub processes: Vec<SessionProcess>,
    /// The output asked for; absent when none was, and when the session
    /// no longer knows the process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ProcessOutput>,
}

/// One background process of a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionProcess {
    /// The id the session's `process` tool names it by.
    pub id: String,
    /// The command line it runs.
    pub command: String,
    pub state: ProcessState,
    /// Seconds since it started; for one that ended, until it ended.
    pub elapsed_seconds: u64,
    /// Seconds since its last output. Absent before any output and once
    /// it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_seconds: Option<u64>,
    /// Its exit code, once it ended with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Where a background process is. Protocol minors may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProcessState {
    Starting,
    Running,
    /// It exited by itself.
    Exited,
    /// The session stopped it.
    Terminated,
    /// It ran past its time limit and was stopped.
    TimedOut,
    FailedToStart,
    /// A state a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A process and the output the session retains of it from a cursor on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessOutput {
    /// The process as it is now, also once it left
    /// [`SessionProcessesResponse::processes`].
    pub process: SessionProcess,
    /// The cursor of the oldest chunk the session still retains. Above 0,
    /// earlier output was evicted, and a client drops the chunks it holds
    /// below it.
    pub first_cursor: u64,
    /// What to send as [`ProcessOutputParams::from`] to read on from here.
    pub next_cursor: u64,
    /// The retained chunks from [`ProcessOutputParams::from`] on, in order.
    pub chunks: Vec<ProcessChunk>,
}

/// One piece of a process's output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessChunk {
    /// Its place in the process's output; cursors only grow.
    pub cursor: u64,
    pub stream: ProcessStream,
    pub text: String,
}

/// The stream a chunk was written to. Protocol minors may add streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProcessStream {
    Stdout,
    Stderr,
    /// A stream a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
