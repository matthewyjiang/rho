//! Click-to-attach actions for subagent rows in the activity rail.

use std::{
    io,
    process::{Command, Stdio},
    time::Instant,
};

use super::{
    clipboard::CopyOutcome, subagent_panel::SubagentAttachTarget, text_selection::CopyNotice, App,
};
use crate::herdr::HerdrReporter;

/// Where a subagent attach request should land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AttachDestination {
    /// Split a new Herdr pane beside this one and run the attach command there.
    HerdrPane { pane_id: String },
    /// No Herdr host: hand the command to the user through the clipboard.
    Clipboard,
}

/// Opens a terminal pane running a command.
///
/// Implementors must not block longer than a local IPC round trip; the TUI calls
/// this on the input thread.
pub(super) trait SubagentPaneOpener {
    fn open(&mut self, pane_id: &str, command: &str) -> io::Result<()>;
}

/// Opens panes by shelling out to the `herdr` CLI.
#[derive(Debug, Default)]
pub(super) struct HerdrCliPaneOpener;

impl SubagentPaneOpener for HerdrCliPaneOpener {
    fn open(&mut self, pane_id: &str, command: &str) -> io::Result<()> {
        open_herdr_pane(pane_id, command)
    }
}

/// Command a user runs to watch delegated run `run_id`.
///
/// `run_id` is a validated 6-char hex id, so it needs no shell quoting.
pub(super) fn attach_command(run_id: &str) -> String {
    format!("rho attach {run_id}")
}

/// Command to run inside a Herdr pane.
///
/// Prefers this process's executable so a cargo-built or downloaded binary still
/// starts when `rho` is not on `PATH`.
pub(super) fn pane_attach_command(run_id: &str) -> String {
    match std::env::current_exe() {
        Ok(path) => {
            let path = path.display().to_string();
            if needs_shell_quoting(&path) {
                format!("{} attach {run_id}", shell_single_quote(&path))
            } else {
                format!("{path} attach {run_id}")
            }
        }
        Err(_) => attach_command(run_id),
    }
}

/// Choose clipboard vs Herdr based on whether this Rho instance is hosted.
pub(super) fn destination(herdr: &HerdrReporter) -> AttachDestination {
    match herdr.pane_id() {
        Some(pane_id) => AttachDestination::HerdrPane {
            pane_id: pane_id.to_string(),
        },
        None => AttachDestination::Clipboard,
    }
}

/// Short hover hint shown on the right edge of a subagent row.
pub(super) fn action_hint(destination: &AttachDestination) -> &'static str {
    match destination {
        AttachDestination::HerdrPane { .. } => "open pane",
        AttachDestination::Clipboard => "copy attach",
    }
}

impl App {
    pub(super) fn subagent_attach_destination(&self) -> AttachDestination {
        destination(&self.info.services.herdr)
    }

    pub(super) fn subagent_action_hint(&self) -> &'static str {
        action_hint(&self.subagent_attach_destination())
    }

    pub(super) fn activate_subagent_row(&mut self, target: &SubagentAttachTarget, now: Instant) {
        let command = attach_command(&target.run_id);
        match self.subagent_attach_destination() {
            AttachDestination::HerdrPane { pane_id } => {
                let pane_command = pane_attach_command(&target.run_id);
                match self.pane_opener.open(&pane_id, &pane_command) {
                    Ok(()) => self.notify_status(format!(
                        "opened a herdr pane attached to {} {}",
                        target.agent_id, target.run_id
                    )),
                    // A dead socket or a stale pane id must not lose the id.
                    Err(error) => {
                        self.notify_status(format!("herdr pane failed: {error}"));
                        self.copy_attach_command(&command, now);
                    }
                }
            }
            AttachDestination::Clipboard => self.copy_attach_command(&command, now),
        }
    }

    fn copy_attach_command(&mut self, command: &str, now: Instant) {
        let outcome = self.clipboard.copy(command);
        let message = match &outcome {
            Ok(CopyOutcome::Confirmed) => format!("copied attach command: {command}"),
            Ok(CopyOutcome::SentToTerminal) => {
                format!("sent attach command to the terminal: {command}")
            }
            Err(error) => format!("copy failed ({error}); attach with: {command}"),
        };
        self.history
            .set_copy_notice(Some(CopyNotice::from_copy_result(
                outcome,
                command.chars().count(),
                now,
            )));
        self.notify_status(message);
    }
}

fn open_herdr_pane(pane_id: &str, command: &str) -> io::Result<()> {
    let split = Command::new("herdr")
        .args([
            "pane",
            "split",
            pane_id,
            "--direction",
            "right",
            "--no-focus",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()?;
    if !split.status.success() {
        return Err(io::Error::other(format!(
            "herdr pane split exited with {}",
            split.status
        )));
    }
    let new_pane_id = pane_id_from_split_response(&split.stdout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "herdr pane split returned no pane id",
        )
    })?;

    let run = Command::new("herdr")
        .args(["pane", "run", &new_pane_id, command])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !run.status.success() {
        return Err(io::Error::other(format!(
            "herdr pane run exited with {}",
            run.status
        )));
    }
    Ok(())
}

/// Extract the new pane id from a `herdr pane split` JSON response.
pub(super) fn pane_id_from_split_response(stdout: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    value
        .pointer("/result/pane/pane_id")
        .and_then(|id| id.as_str())
        .map(str::to_string)
}

fn needs_shell_quoting(value: &str) -> bool {
    value
        .chars()
        .any(|ch| ch.is_whitespace() || "\"'$&|;<>()\\".contains(ch))
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
#[path = "subagent_attach_tests.rs"]
mod tests;
