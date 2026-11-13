//! The JSON Lines stream of `rho run --output jsonl`, at `schema_version` 2
//! (#1669): the daemon protocol's session events (`docs/protocol.md`)
//! between a `run_started` and a `run_ended` record, so a foreground run
//! and a client of a daemon session read the same events.

use std::{
    io::{self, Write},
    path::Path,
    time::Duration,
};

use rho_protocol::SessionEvent;
use serde::Serialize;

const SCHEMA_VERSION: u8 = 2;

/// Parses a human-readable CLI duration such as `30s` or `20m`.
pub(crate) fn parse_duration(value: &str) -> Result<Duration, String> {
    humantime::parse_duration(value).map_err(|error| format!("invalid duration '{value}': {error}"))
}

/// Why a run ended, as its `run_ended` record says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalReason {
    Completed,
    MaxSteps,
    Timeout,
    Interrupted,
    /// A client cancelled the turn `rho attach` followed.
    Cancelled,
    /// `rho attach` detached on a signal; the turn runs on.
    Detached,
    Authentication,
    ProviderError,
    ToolHostError,
    ConfigurationError,
    OutputError,
    OtherError,
}

/// How a run ended: the stream's `run_ended` record and the process's exit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RunEnd {
    pub(crate) reason: TerminalReason,
    pub(crate) exit_code: u8,
    /// What went wrong; `None` for a run that completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

impl RunEnd {
    /// A run that completed: exit code 0, nothing to say.
    pub(crate) fn completed() -> Self {
        Self {
            reason: TerminalReason::Completed,
            exit_code: 0,
            message: None,
        }
    }
}

/// One line of the stream.
#[derive(Serialize)]
struct Line<'a> {
    schema_version: u8,
    seq: u64,
    #[serde(flatten)]
    record: Record<'a>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record<'a> {
    /// The run's session opened.
    RunStarted {
        session: &'a str,
        workspace: &'a Path,
    },
    /// One of the session's events.
    Event { event: &'a SessionEvent },
    /// The run ended: the stream's last line.
    RunEnded(&'a RunEnd),
}

/// One run's stream on `out` (stdout unless a test lends another). Numbers
/// its lines from 1 and flushes each. The first write that fails is the
/// stream's [`Self::failure`], which fails the run; nothing is written
/// after it.
pub(crate) struct JsonlStream<W = io::Stdout> {
    out: W,
    seq: u64,
    failure: Option<String>,
}

impl JsonlStream {
    pub(crate) fn stdout() -> Self {
        Self::new(io::stdout())
    }
}

impl<W: Write> JsonlStream<W> {
    pub(crate) fn new(out: W) -> Self {
        Self {
            out,
            seq: 0,
            failure: None,
        }
    }

    /// Writes `run_started` for the session `session` in `workspace`.
    pub(crate) fn run_started(&mut self, session: &str, workspace: &Path) {
        self.write(Record::RunStarted { session, workspace });
    }

    pub(crate) fn event(&mut self, event: &SessionEvent) {
        self.write(Record::Event { event });
    }

    /// Writes the last line: how the run ended.
    pub(crate) fn end(&mut self, end: &RunEnd) {
        self.write(Record::RunEnded(end));
    }

    /// Why the stream stopped, once a write failed.
    pub(crate) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn write(&mut self, record: Record<'_>) {
        if self.failure.is_some() {
            return;
        }
        self.seq += 1;
        let line = Line {
            schema_version: SCHEMA_VERSION,
            seq: self.seq,
            record,
        };
        let written = serde_json::to_writer(&mut self.out, &line)
            .map_err(io::Error::other)
            .and_then(|()| self.out.write_all(b"\n"))
            .and_then(|()| self.out.flush());
        if let Err(error) = written {
            self.failure = Some(format!("could not write JSONL output: {error}"));
        }
    }
}

#[cfg(test)]
#[path = "automation_protocol_tests.rs"]
mod tests;
