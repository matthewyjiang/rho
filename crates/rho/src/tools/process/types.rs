use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Whether a started process holds the prompt cache warm while it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum KeepWarm {
    #[default]
    Off,
    On,
    /// Requested, but the bound model route cannot send a capped ping, so
    /// nothing holds and the start result says so.
    Unavailable,
}

/// The session's bound model route, as keep-warm needs to know it: whether
/// it can send a capped ping, and whether it still holds the prefix the last
/// request cached.
///
/// The session's host owns it and updates it whenever the provider changes;
/// `process start` reads it so the model is told whether warming will run.
#[derive(Clone, Debug, Default)]
pub(crate) struct CacheWarmRoute(std::sync::Arc<RouteState>);

#[derive(Debug, Default)]
struct RouteState {
    supported: std::sync::atomic::AtomicBool,
    /// Bumped each time the route stops holding the cached prefix.
    prefix_epoch: std::sync::atomic::AtomicU64,
}

impl CacheWarmRoute {
    pub(crate) fn set_supported(&self, supported: bool) {
        self.0
            .supported
            .store(supported, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn is_supported(&self) -> bool {
        self.0.supported.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The route no longer holds the prefix the last request cached: the
    /// provider was replaced, or a setting that is part of the prefix
    /// changed. A warmer that reads [`Self::prefix_epoch`] starts over.
    #[cfg_attr(
        not(unix),
        allow(dead_code, reason = "only the Unix daemon's warmer asks")
    )]
    pub(crate) fn prefix_dropped(&self) {
        self.0
            .prefix_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Changes whenever [`Self::prefix_dropped`] was called.
    #[cfg_attr(
        not(unix),
        allow(dead_code, reason = "only the Unix daemon's warmer asks")
    )]
    pub(crate) fn prefix_epoch(&self) -> u64 {
        self.0
            .prefix_epoch
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

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
#[derive(Clone, Copy, Debug, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    Running,
    Exited,
    Terminated,
    TimedOut,
    FailedToStart,
}

impl State {
    pub(crate) const fn is_live(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    pub(crate) const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Terminated => "terminated",
            Self::TimedOut => "timed_out",
            Self::FailedToStart => "failed_to_start",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema, PartialEq)]
pub struct Chunk {
    pub cursor: u64,
    pub stream: Stream,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Snapshot {
    pub process_id: String,
    pub command: String,
    pub state: State,
    pub runtime_seconds: f64,
    pub first_cursor: u64,
    pub next_cursor: u64,
    pub available_cursor: u64,
    pub truncated: bool,
    pub output_pending: bool,
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

/// Host-UI-only retained output for one process. Not a tool action.
///
/// Does not mark the process observed or advance any agent-visible poll cursor.
#[derive(Clone, Debug)]
pub(crate) struct HostProcessView {
    pub(crate) snapshot: Snapshot,
    pub(crate) elapsed_seconds: u64,
    /// Seconds since the most recent output chunk. None when the process has
    /// produced no output yet, and None for terminal-state rows.
    pub(crate) quiet_seconds: Option<u64>,
}

/// Host-side view of one live process for UI rails. Not a tool action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveProcessSummary {
    pub(crate) process_id: String,
    pub(crate) command: String,
    pub(crate) state: State,
    pub(crate) elapsed_seconds: u64,
    /// Seconds since the most recent output chunk. None when the process has
    /// produced no output yet, and None for terminal-state rows (the UI shows
    /// a verdict instead).
    pub(crate) quiet_seconds: Option<u64>,
    /// Populated for terminal rows from the process exit code. None for live rows.
    pub(crate) exit_code: Option<i32>,
}
