//! What each action takes and returns. Front ends publish the params as
//! their input schemas and return the results as JSON, so every front end
//! answers the same call the same way.

use std::{collections::HashMap, num::NonZeroUsize, path::PathBuf, time::Duration};

use rho_protocol::{
    Cursor, InputRequest, JobId, JobState, JobSummary, Seq, SessionId, SteerId, StreamId,
    TurnOutcome,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ControlError;

/// How many sessions `list` returns by default.
//
// One screenful a model reads at once: 20 entries of about 200 bytes is
// about 4 KB, `read`'s default window.
const DEFAULT_LIST_LIMIT: usize = 20;
/// The most sessions one `list` returns: the daemon's own page limit for
/// its session catalog.
const MAX_LIST_LIMIT: usize = 1000;
/// `read`'s default window, in characters: the native `sessions` tool's.
const DEFAULT_READ_CHARS: usize = 4096;
/// `read`'s largest window: rho's default tool output limit
/// (`rho_tools::DEFAULT_MAX_OUTPUT_BYTES`).
const MAX_READ_CHARS: usize = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
/// How long `wait` waits when the call names no timeout.
const DEFAULT_WAIT_SECS: u64 = 30;
/// The longest one `wait` waits, under the hosts' own tool-call limits.
//
// Codex's default `tool_timeout_sec`, 60 s, is the smallest default among
// the hosts the docs cover; 50 s leaves the reply time to arrive. Remote
// hosts behind an HTTP time limit call again with the cursor.
const MAX_WAIT_SECS: u64 = 50;

/// Params of `list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    /// Only sessions in this workspace, an absolute path.
    #[serde(default)]
    pub(crate) workspace: Option<PathBuf>,
    /// Only sessions in this state.
    #[serde(default)]
    pub(crate) state: Option<JobState>,
    /// The most sessions to return, most recently changed first: 20 when
    /// absent, at most 1000.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

impl ListParams {
    pub(super) fn limit(&self) -> Result<usize, ControlError> {
        let limit = self.limit.map_or(DEFAULT_LIST_LIMIT, NonZeroUsize::get);
        if limit > MAX_LIST_LIMIT {
            return Err(ControlError::Invalid(format!(
                "list limit: limit {MAX_LIST_LIMIT}, asked {limit}"
            )));
        }
        Ok(limit)
    }
}

/// Result of `list`.
#[derive(Debug, Serialize)]
pub(crate) struct ListResult {
    pub(crate) sessions: Vec<SessionEntry>,
    /// Whether more sessions matched than `limit` let through.
    pub(crate) more: bool,
}

/// One session, as `list` reports it.
#[derive(Debug, Serialize)]
pub(crate) struct SessionEntry {
    pub(crate) session: SessionId,
    pub(crate) workspace: PathBuf,
    pub(crate) state: JobState,
    /// Unix time, in seconds, of the session's last state change.
    pub(crate) updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// The session whose agent started this one (#1714).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<SessionId>,
}

impl SessionEntry {
    /// The entry for a session job, whose parent is the session `writers`
    /// says its parent job writes; `None` for a job that writes no session.
    pub(super) fn of_job(job: JobSummary, writers: &HashMap<JobId, SessionId>) -> Option<Self> {
        Some(Self {
            parent: job.parent.and_then(|parent| writers.get(&parent).cloned()),
            session: job.session?,
            workspace: job.workspace,
            state: job.state,
            updated_at: job.updated_at,
            error: job.error,
        })
    }
}

/// Params of `read`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadParams {
    pub(crate) session: SessionId,
    /// The first character to return; 0 when absent. Pass the last read's
    /// `next_start` to page on.
    #[serde(default)]
    pub(crate) start: usize,
    /// How many characters to return: 4096 when absent, at most 64000.
    #[serde(default)]
    pub(crate) chars: Option<NonZeroUsize>,
}

/// Which characters of a transcript `read` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Window {
    start: usize,
    chars: usize,
}

impl ReadParams {
    pub(super) fn window(&self) -> Result<Window, ControlError> {
        let chars = self.chars.map_or(DEFAULT_READ_CHARS, NonZeroUsize::get);
        if chars > MAX_READ_CHARS {
            return Err(ControlError::Invalid(format!(
                "read window: limit {MAX_READ_CHARS} characters, asked {chars}"
            )));
        }
        Ok(Window {
            start: self.start,
            chars,
        })
    }
}

/// Result of `read`: characters `start..end` of the transcript's
/// `total_chars`.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReadResult {
    pub(crate) session: SessionId,
    pub(crate) text: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) total_chars: usize,
    /// Where the next page starts; absent on the last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_start: Option<usize>,
}

impl ReadResult {
    pub(super) fn window(session: SessionId, transcript: &str, window: Window) -> Self {
        let total_chars = transcript.chars().count();
        let start = window.start.min(total_chars);
        let end = start.saturating_add(window.chars).min(total_chars);
        Self {
            session,
            text: transcript.chars().skip(start).take(end - start).collect(),
            start,
            end,
            total_chars,
            next_start: (end < total_chars).then_some(end),
        }
    }
}

/// Params of `new`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewParams {
    /// The session's workspace, an absolute path to an existing directory.
    pub(crate) workspace: PathBuf,
    /// The first prompt, which starts the session's first turn.
    pub(crate) prompt: String,
    /// The agent the session runs as, by id, one of the workspace's; the
    /// default agent when absent.
    #[serde(default)]
    pub(crate) agent: Option<String>,
    /// The model, as `provider/model` or a model id one provider alone
    /// offers; the workspace's default when absent.
    #[serde(default)]
    pub(crate) model: Option<String>,
}

/// Result of `new`: the session runs its first turn.
#[derive(Debug, Serialize)]
pub(crate) struct NewResult {
    pub(crate) session: SessionId,
    pub(crate) workspace: PathBuf,
}

/// Params of `prompt`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PromptParams {
    pub(crate) session: SessionId,
    pub(crate) text: String,
}

/// Result of `prompt`: the turn began.
#[derive(Debug, Serialize)]
pub(crate) struct PromptResult {
    pub(crate) session: SessionId,
}

/// Params of `steer`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SteerParams {
    pub(crate) session: SessionId,
    /// Input for the running turn's next model request.
    pub(crate) text: String,
}

/// Result of `steer`: the running turn took the input.
#[derive(Debug, Serialize)]
pub(crate) struct SteerResult {
    pub(crate) id: SteerId,
}

/// Params of `cancel`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CancelParams {
    pub(crate) session: SessionId,
}

/// Result of `cancel`: the turn in progress, if any, was asked to stop.
#[derive(Debug, Serialize)]
pub(crate) struct CancelResult {}

/// Params of `wait`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitParams {
    pub(crate) session: SessionId,
    /// How long to wait, in seconds: 30 when absent, at most 50.
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
    /// The `cursor` the last `wait` on this session returned, to resume
    /// after what it saw.
    #[serde(default)]
    pub(crate) cursor: Option<String>,
}

impl WaitParams {
    pub(super) fn timeout(&self) -> Result<Duration, ControlError> {
        let secs = self.timeout_secs.unwrap_or(DEFAULT_WAIT_SECS);
        if secs > MAX_WAIT_SECS {
            return Err(ControlError::Invalid(format!(
                "wait timeout: limit {MAX_WAIT_SECS} s, asked {secs} s; call again with the cursor to wait longer"
            )));
        }
        Ok(Duration::from_secs(secs))
    }

    pub(super) fn cursor(&self) -> Result<Option<Cursor>, ControlError> {
        self.cursor.as_deref().map(parse_cursor).transpose()
    }
}

/// Result of `wait`.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WaitResult {
    pub(crate) session: SessionId,
    pub(crate) state: JobState,
    /// Whether the session's turn stopped running: it ended, or it waits
    /// for a person to answer `inputs`.
    pub(crate) done: bool,
    /// How the turn ended, the final answer included, when it ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<TurnOutcome>,
    /// What the turn waits on a person for. Only a person answers these,
    /// from a rho frontend; no session-control action can.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) inputs: Vec<InputRequest>,
    /// Pass to the next `wait` on this session to resume after what this
    /// one saw; absent when it saw nothing to resume after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cursor: Option<String>,
}

/// A cursor as `wait` hands it out: `<stream>:<seq>`.
pub(super) fn format_cursor(cursor: &Cursor) -> String {
    format!("{}:{}", cursor.stream, cursor.seq.0)
}

fn parse_cursor(text: &str) -> Result<Cursor, ControlError> {
    let invalid = || ControlError::Invalid(format!("cursor {text:?} is not one `wait` returned"));
    let (stream, seq) = text.rsplit_once(':').ok_or_else(invalid)?;
    let seq = seq.parse().map_err(|_| invalid())?;
    if stream.is_empty() {
        return Err(invalid());
    }
    Ok(Cursor {
        stream: StreamId::new(stream),
        seq: Seq(seq),
    })
}

#[cfg(test)]
#[path = "params_tests.rs"]
mod tests;
