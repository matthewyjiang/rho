//! Program Status Protocol (OSC 7501): tells the host terminal whether Rho is
//! idle, working, blocked on the user, done, or failed, so terminals and agent
//! dashboards can show it without scraping the screen.
//!
//! Spec: <https://www.superlogical.com/rex/docs/build/program-status> (rev 0.2).
//! Rho reports only the root record. The startup terminal probe sends
//! [`QUERY`]; reports go out only when the terminal answered it, unless
//! `RHO_PROGRAM_STATUS=1` forces them on or `RHO_PROGRAM_STATUS=0` turns them off.
//!
//! Messages are fixed wait text, the sign-in hint, a blocked goal's reason
//! (written by the model), or the first line of a turn error. Prompts and
//! assistant replies are never reported.

use base64::Engine as _;

/// Feature detection query. A supporting terminal replies with the same body.
pub(super) const QUERY: &[u8] = b"\x1b]7501;?\x1b\\";
/// Removes every record Rho reported on this terminal.
const CLEAR: &[u8] = b"\x1b]7501;state=clear\x1b\\";
/// Stable `app` value; reports replace the whole record, so every one repeats it.
const APP: &str = "rho";
/// Decoded `msg` cap from the spec's limits table. Its base64 stays under the
/// 2732-byte encoded cap, and the whole report under the 4096-byte sequence cap.
const MAX_MESSAGE_BYTES: usize = 2048;

/// Whether reports reach the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProgramStatusSupport {
    Supported,
    Unsupported,
}

impl ProgramStatusSupport {
    /// Support as answered by one OSC reply body (the bytes between `ESC ]`
    /// and the terminator). Later spec revisions may add pairs after the `?`.
    pub(super) fn from_reply(body: &str) -> Option<Self> {
        body.starts_with("7501;?").then_some(Self::Supported)
    }

    /// Applies `RHO_PROGRAM_STATUS`: `1` reports without a reply, for example
    /// when a slow SSH link misses the probe deadline; `0` never reports.
    pub(super) fn with_override(self, value: Option<&str>) -> Self {
        match value {
            Some("1") => Self::Supported,
            Some("0") => Self::Unsupported,
            _ => self,
        }
    }
}

/// What a blocked session waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockedKind {
    /// Approval to do something.
    Permission,
    /// An answer the user must give.
    Question,
    /// A login or credential.
    Auth,
}

impl BlockedKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// One root-record report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ProgramStatus {
    /// At rest: started, or the last turn was interrupted or cancelled.
    Idle,
    Working,
    /// The last turn finished.
    Done,
    Blocked {
        kind: Option<BlockedKind>,
        message: String,
    },
    /// The last turn failed and was not retried.
    Error {
        message: String,
    },
}

impl ProgramStatus {
    fn encode(&self) -> Vec<u8> {
        let (state, kind, message) = match self {
            Self::Idle => ("idle", None, None),
            Self::Working => ("working", None, None),
            Self::Done => ("done", None, None),
            Self::Blocked { kind, message } => ("blocked", *kind, Some(message.as_str())),
            Self::Error { message } => ("error", None, Some(message.as_str())),
        };
        let mut body = format!("state={state}:app={APP}");
        if let Some(kind) = kind {
            body.push_str(":kind=");
            body.push_str(kind.as_str());
        }
        if let Some(line) = message.and_then(message_line) {
            body.push_str(":msg=");
            base64::engine::general_purpose::STANDARD.encode_string(line, &mut body);
        }
        format!("\x1b]7501;{body}\x1b\\").into_bytes()
    }
}

/// The first non-blank line of `text`, cut to the `msg` cap. Terminals drop a
/// whole report whose message holds a control character, so those become spaces.
fn message_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut line: String = line
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let mut end = line.len().min(MAX_MESSAGE_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line.truncate(end);
    Some(line)
}

/// Support gating and repeat suppression for terminal reports. Returns bytes
/// instead of writing so callers choose when to write.
#[derive(Debug)]
pub(super) struct ProgramStatusReporter {
    support: ProgramStatusSupport,
    sent: Option<ProgramStatus>,
    /// Retained across invalidation so teardown still clears our record.
    reported: bool,
}

impl ProgramStatusReporter {
    pub(super) fn new(support: ProgramStatusSupport) -> Self {
        Self {
            support,
            sent: None,
            reported: false,
        }
    }

    /// Bytes for `status`, or `None` when unsupported or already reported.
    pub(super) fn report(&mut self, status: ProgramStatus) -> Option<Vec<u8>> {
        if self.support == ProgramStatusSupport::Unsupported || self.sent.as_ref() == Some(&status)
        {
            return None;
        }
        let bytes = status.encode();
        self.sent = Some(status);
        self.reported = true;
        Some(bytes)
    }

    /// A suspended child may replace or clear the terminal's root record.
    pub(super) fn invalidate(&mut self) {
        self.sent = None;
    }

    /// Bytes that remove Rho's record on exit, so a `done` the user already
    /// saw does not outlive the session. `None` when nothing was reported.
    pub(super) fn clear(&mut self) -> Option<Vec<u8>> {
        self.sent = None;
        std::mem::take(&mut self.reported).then(|| CLEAR.to_vec())
    }
}

#[cfg(test)]
#[path = "program_status_tests.rs"]
mod tests;
