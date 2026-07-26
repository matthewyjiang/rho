//! Click-to-attach actions for subagent rows in the activity rail.

use std::{io, path::Path, time::Instant};

use futures_util::FutureExt;

use super::{
    clipboard::CopyOutcome, subagent_panel::SubagentAttachTarget, text_selection::CopyNotice, App,
};
use crate::herdr::HerdrReporter;

/// Where a subagent attach request should land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttachDestination {
    /// Split a new Herdr pane beside this one and run the attach command there.
    HerdrPane,
    /// No Herdr host: hand the command to the user through the clipboard.
    Clipboard,
}

pub(super) struct PendingSubagentAttach {
    target: SubagentAttachTarget,
    clipboard_command: String,
    handle: tokio::task::JoinHandle<io::Result<()>>,
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
/// starts when `rho` is not on `PATH`. Every argument is quoted because Herdr
/// submits this string to the pane's shell.
pub(super) fn pane_attach_command(run_id: &str) -> String {
    let executable = std::env::current_exe().ok();
    pane_attach_command_for_executable(executable.as_deref(), run_id)
}

fn pane_attach_command_for_executable(executable: Option<&Path>, run_id: &str) -> String {
    let executable = executable
        .map(|path| path.to_string_lossy())
        .unwrap_or_else(|| "rho".into());
    format!(
        "{} {} {}",
        shell_single_quote(&executable),
        shell_single_quote("attach"),
        shell_single_quote(run_id)
    )
}

/// Choose clipboard vs Herdr based on whether this Rho instance is hosted.
pub(super) fn destination(herdr: &HerdrReporter) -> AttachDestination {
    if herdr.is_enabled() {
        AttachDestination::HerdrPane
    } else {
        AttachDestination::Clipboard
    }
}

/// Short hover hint shown on the right edge of a subagent row.
pub(super) fn action_hint(destination: AttachDestination) -> &'static str {
    match destination {
        AttachDestination::HerdrPane => "open pane",
        AttachDestination::Clipboard => "copy attach",
    }
}

impl App {
    pub(super) fn subagent_attach_destination(&self) -> AttachDestination {
        destination(&self.info.services.herdr)
    }

    pub(super) fn subagent_action_hint(&self) -> &'static str {
        action_hint(self.subagent_attach_destination())
    }

    pub(super) fn activate_subagent_row(&mut self, target: &SubagentAttachTarget, now: Instant) {
        let clipboard_command = attach_command(&target.run_id);
        match self.subagent_attach_destination() {
            AttachDestination::HerdrPane => {
                if self
                    .pending_subagent_attaches
                    .iter()
                    .any(|pending| pending.target.run_id == target.run_id)
                {
                    return;
                }
                let herdr = self.info.services.herdr.clone();
                let pane_command = pane_attach_command(&target.run_id);
                let handle =
                    tokio::spawn(async move { herdr.open_sibling_pane(&pane_command).await });
                self.pending_subagent_attaches.push(PendingSubagentAttach {
                    target: target.clone(),
                    clipboard_command,
                    handle,
                });
                self.notify_status(format!(
                    "opening a herdr pane for {} {}",
                    target.agent_id, target.run_id
                ));
            }
            AttachDestination::Clipboard => {
                let message = self.copy_attach_command(&clipboard_command, now);
                self.notify_status(message);
            }
        }
    }

    pub(super) fn poll_pending_subagent_attaches(&mut self, now: Instant) -> bool {
        let mut changed = false;
        while let Some(index) = self
            .pending_subagent_attaches
            .iter()
            .position(|pending| pending.handle.is_finished())
        {
            let pending = self.pending_subagent_attaches.remove(index);
            let result = pending
                .handle
                .now_or_never()
                .expect("finished subagent attach task has a result");
            match result {
                Ok(Ok(())) => self.notify_status(format!(
                    "opened a herdr pane attached to {} {}",
                    pending.target.agent_id, pending.target.run_id
                )),
                Ok(Err(error)) => {
                    let copy_message = self.copy_attach_command(&pending.clipboard_command, now);
                    self.notify_status(format!("herdr pane failed ({error}); {copy_message}"));
                }
                Err(error) => {
                    let copy_message = self.copy_attach_command(&pending.clipboard_command, now);
                    self.notify_status(format!("herdr pane task failed ({error}); {copy_message}"));
                }
            }
            changed = true;
        }
        changed
    }

    pub(super) fn has_pending_subagent_attach(&self) -> bool {
        !self.pending_subagent_attaches.is_empty()
    }

    fn copy_attach_command(&mut self, command: &str, now: Instant) -> String {
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
        message
    }
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
#[path = "subagent_attach_tests.rs"]
mod tests;
