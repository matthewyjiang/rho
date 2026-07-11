use serde::Serialize;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ProcessLimits {
    pub max_live: usize,
    pub max_records: usize,
    pub max_bytes: usize,
    pub max_chunks: usize,
    pub retention: Duration,
}
impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            max_live: 16,
            max_records: 64,
            max_bytes: 1024 * 1024,
            max_chunks: 8192,
            retention: Duration::from_secs(30 * 60),
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    Running,
    Exited,
    Terminated,
    TimedOut,
    FailedToStart,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Chunk {
    pub cursor: u64,
    pub stream: Stream,
    pub text: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub process_id: String,
    pub command: String,
    pub state: State,
    pub runtime_seconds: f64,
    pub first_cursor: u64,
    pub next_cursor: u64,
    pub truncated: bool,
    pub chunks: Vec<Chunk>,
    pub exit_code: Option<i32>,
    pub terminal_detail: Option<String>,
}
pub(super) fn terminal(s: State) -> bool {
    matches!(
        s,
        State::Exited | State::Terminated | State::TimedOut | State::FailedToStart
    )
}
