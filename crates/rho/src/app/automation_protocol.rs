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
    Authentication,
    ProviderError,
    ToolHostError,
    ConfigurationError,
    OutputError,
    OtherError,
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
    RunEnded {
        reason: TerminalReason,
        exit_code: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<&'a str>,
    },
}

/// One run's stream on stdout. Numbers its lines from 1 and flushes each.
/// Once a write fails it writes nothing more: the first failure is the one
/// reported, and the run ends with it.
#[derive(Default)]
pub(crate) struct JsonlStream {
    seq: u64,
    /// Whether `run_started` went out.
    opened: bool,
    /// Whether the run's turn began, so the stream owes its `turn_ended`.
    turn: bool,
    broken: bool,
}

impl JsonlStream {
    /// Writes `run_started` for the session `session` in `workspace`, once:
    /// a session that rebinds its agent stays the run's.
    pub(crate) fn open(&mut self, session: &str, workspace: &Path) -> io::Result<()> {
        if std::mem::replace(&mut self.opened, true) {
            return Ok(());
        }
        self.write(Record::RunStarted { session, workspace })
    }

    /// Notes that the run's turn began.
    pub(crate) fn begin_turn(&mut self) {
        self.turn = true;
    }

    /// Whether the run's turn began, so its end is owed as `turn_ended`.
    pub(crate) fn turn_began(&self) -> bool {
        self.turn
    }

    pub(crate) fn event(&mut self, event: &SessionEvent) -> io::Result<()> {
        self.write(Record::Event { event })
    }

    /// Writes the last line: why the run ended, its exit code, and a
    /// message unless it completed.
    pub(crate) fn end(
        &mut self,
        reason: TerminalReason,
        exit_code: u8,
        message: Option<&str>,
    ) -> io::Result<()> {
        self.write(Record::RunEnded {
            reason,
            exit_code,
            message,
        })
    }

    fn write(&mut self, record: Record<'_>) -> io::Result<()> {
        if self.broken {
            return Ok(());
        }
        self.seq += 1;
        let line = Line {
            schema_version: SCHEMA_VERSION,
            seq: self.seq,
            record,
        };
        let written = (|| {
            let mut stdout = io::stdout().lock();
            serde_json::to_writer(&mut stdout, &line).map_err(io::Error::other)?;
            stdout.write_all(b"\n")?;
            stdout.flush()
        })();
        self.broken = written.is_err();
        written
    }
}

#[cfg(test)]
#[path = "automation_protocol_tests.rs"]
mod tests;
