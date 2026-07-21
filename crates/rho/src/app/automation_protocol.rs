//! Versioned JSON Lines wire types for `rho run --output jsonl`.
//!
//! These types stay private so the CLI protocol can evolve independently from
//! the non-exhaustive SDK event API. The adapter owns sequence numbers,
//! assistant-attempt tracking, and the single-terminal-event invariant.

use std::{io, time::Duration};

use serde::Serialize;

const SCHEMA_VERSION: u8 = 1;

/// Parses a human-readable CLI duration such as `30s` or `20m`.
pub(crate) fn parse_duration(value: &str) -> Result<Duration, String> {
    humantime::parse_duration(value).map_err(|error| format!("invalid duration '{value}': {error}"))
}

/// Stable machine-readable reason included in terminal JSONL events.
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

/// One physical JSONL record with protocol metadata and event-specific fields.
#[derive(Debug, Serialize)]
pub(crate) struct WireEvent {
    schema_version: u8,
    seq: u64,
    #[serde(flatten)]
    kind: WireEventKind,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(crate) enum WireEventKind {
    #[serde(rename = "run.started")]
    RunStarted {
        run_id: String,
        session_id: String,
        workspace: String,
    },
    #[serde(rename = "assistant.text_delta")]
    AssistantTextDelta { attempt: u64, text: String },
    #[serde(rename = "assistant.text_reset")]
    AssistantTextReset { attempt: u64 },
    #[serde(rename = "tool.started")]
    ToolStarted { call_id: String, name: String },
    #[serde(rename = "tool.updated")]
    ToolUpdated {
        call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        completed_units: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total_units: Option<u64>,
    },
    #[serde(rename = "tool.finished")]
    ToolFinished { call_id: String, status: ToolStatus },
    #[serde(rename = "run.completed")]
    RunCompleted {
        reason: TerminalReason,
        text: String,
    },
    #[serde(rename = "run.failed")]
    RunFailed {
        reason: TerminalReason,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    #[serde(rename = "run.stopped")]
    RunStopped {
        reason: TerminalReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

/// Stable status for a finished tool call without tool output or error details.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolStatus {
    Success,
    Failure,
    Unavailable,
}

/// Translates SDK events into the independently versioned CLI protocol.
///
/// The adapter omits tool arguments and raw output, assigns run-local sequence
/// numbers, and ignores further SDK events once it creates a terminal event.
pub(crate) struct JsonlAdapter {
    seq: u64,
    attempt: u64,
    session_id: Option<String>,
    workspace: Option<String>,
    terminal: bool,
    assistant_text: String,
}

impl JsonlAdapter {
    pub(crate) fn new() -> Self {
        Self {
            seq: 0,
            attempt: 1,
            session_id: None,
            workspace: None,
            terminal: false,
            assistant_text: String::new(),
        }
    }

    /// Adds identity fields that become available after session startup.
    pub(crate) fn set_run_context(
        &mut self,
        session_id: impl ToString,
        workspace: &std::path::Path,
    ) {
        self.session_id = Some(session_id.to_string());
        self.workspace = Some(workspace.to_string_lossy().into_owned());
    }

    /// Converts a supported SDK event, or returns `None` for events outside
    /// the wire contract.
    pub(crate) fn event(&mut self, event: &rho_sdk::RunEvent) -> Option<WireEvent> {
        use rho_sdk::{RunEvent, ToolCompletion};

        if self.terminal {
            return None;
        }
        let kind = match event {
            RunEvent::Started { run_id, .. } => WireEventKind::RunStarted {
                run_id: run_id.to_string(),
                session_id: self.session_id.clone().unwrap_or_default(),
                workspace: self.workspace.clone().unwrap_or_default(),
            },
            RunEvent::AssistantTextDelta { text } => {
                self.assistant_text.push_str(text);
                WireEventKind::AssistantTextDelta {
                    attempt: self.attempt,
                    text: text.clone(),
                }
            }
            RunEvent::ProviderStreamReset { .. } => {
                let attempt = self.attempt;
                self.attempt += 1;
                self.assistant_text.clear();
                WireEventKind::AssistantTextReset { attempt }
            }
            RunEvent::ToolStarted { call_id, name, .. } => WireEventKind::ToolStarted {
                call_id: call_id.to_string(),
                name: name.clone(),
            },
            RunEvent::ToolUpdated { call_id, progress } => {
                let completed_units = progress.completed_units();
                let total_units = progress.total_units();
                if completed_units.is_none() && total_units.is_none() {
                    return None;
                }
                WireEventKind::ToolUpdated {
                    call_id: call_id.to_string(),
                    completed_units,
                    total_units,
                }
            }
            RunEvent::ToolFinished { call_id, result } => WireEventKind::ToolFinished {
                call_id: call_id.to_string(),
                status: match result {
                    ToolCompletion::Success(_) => ToolStatus::Success,
                    ToolCompletion::Failure(_) => ToolStatus::Failure,
                    ToolCompletion::Unavailable => ToolStatus::Unavailable,
                    _ => ToolStatus::Failure,
                },
            },
            RunEvent::Completed { .. } | RunEvent::Cancelled { .. } | RunEvent::Failed { .. } => {
                return None;
            }
            _ => return None,
        };
        Some(self.next(kind))
    }

    /// Returns text from the current provider attempt for a terminal event.
    pub(crate) fn partial_text(&self) -> Option<String> {
        (!self.assistant_text.is_empty()).then(|| self.assistant_text.clone())
    }

    /// Creates the successful terminal event and closes the adapter.
    pub(crate) fn completed(&mut self, text: String) -> WireEvent {
        self.terminal(WireEventKind::RunCompleted {
            reason: TerminalReason::Completed,
            text,
        })
    }

    /// Creates a stopped terminal event and closes the adapter.
    pub(crate) fn stopped(&mut self, reason: TerminalReason, text: Option<String>) -> WireEvent {
        self.terminal(WireEventKind::RunStopped { reason, text })
    }

    /// Creates a failed terminal event and closes the adapter.
    pub(crate) fn failed(
        &mut self,
        reason: TerminalReason,
        message: String,
        text: Option<String>,
    ) -> WireEvent {
        self.terminal(WireEventKind::RunFailed {
            reason,
            message,
            text,
        })
    }

    fn terminal(&mut self, kind: WireEventKind) -> WireEvent {
        debug_assert!(!self.terminal, "JSONL terminal event emitted twice");
        self.terminal = true;
        self.next(kind)
    }

    fn next(&mut self, kind: WireEventKind) -> WireEvent {
        self.seq += 1;
        WireEvent {
            schema_version: SCHEMA_VERSION,
            seq: self.seq,
            kind,
        }
    }
}

/// Writes and flushes one event as one physical JSONL line.
pub(crate) fn write_event(writer: &mut impl io::Write, event: &WireEvent) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, event).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
#[path = "automation_protocol_tests.rs"]
mod tests;
