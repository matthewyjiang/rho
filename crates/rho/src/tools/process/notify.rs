//! Parent-session delivery for finished background processes.
//!
//! Mirrors workflow and delegated-agent notifications: the manager stores a
//! terminal snapshot, and the TUI drains unobserved terminals at the next
//! turn boundary.

use std::time::Duration;

use super::types::State;
use crate::presentation::{
    NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
    NotificationVisibility,
};

const MODEL_NOTIFICATION_BYTES: usize = 16 * 1024;
const OUTPUT_EXCERPT_BYTES: usize = 4 * 1024;
const NOTIFICATION_HEADER: &str = "[process notification]\n\nProcess status:\n";
const NOTIFICATION_FOOTER: &str = "\n\nAny omitted details remain available through the process tool (`poll`). This is an automated notification, not a user message. Fold the results into your ongoing work; do not poll in a loop.\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessNotification {
    pub(crate) process_id: String,
    pub(crate) command: String,
    pub(crate) state: State,
    pub(crate) exit_code: Option<i32>,
    pub(crate) output: String,
    pub(crate) terminal_detail: Option<String>,
    pub(crate) elapsed: Duration,
}

impl ProcessNotification {
    /// Transcript card for one finished process: outcome and command up front,
    /// run time beneath, output as the body, identity in the expandable details.
    pub(crate) fn card(&self) -> NotificationCard {
        let (outcome, tone) = match (self.state, self.exit_code) {
            (State::Exited, Some(0)) => ("Completed".to_string(), NotificationTone::Success),
            (State::Exited, Some(code)) => {
                (format!("Failed (exit {code})"), NotificationTone::Error)
            }
            (State::Exited, None) => ("Exited without code".into(), NotificationTone::Warning),
            (State::Terminated, _) => ("Stopped".into(), NotificationTone::Accent),
            (State::TimedOut, _) => ("Timed out".into(), NotificationTone::Warning),
            (State::FailedToStart, _) => ("Failed to start".into(), NotificationTone::Error),
            (State::Starting | State::Running, _) => ("Running".into(), NotificationTone::Accent),
        };
        let mut sections = Vec::new();
        if let Some(detail) = &self.terminal_detail {
            sections.push(detail.clone());
        }
        if !self.output.trim().is_empty() {
            sections.push(fenced_output(self.output.trim_end()));
        }
        let body = sections.join("\n\n");
        NotificationCard {
            title: format!("{outcome} · {}", self.command),
            sender: "process".into(),
            recipient: "parent".into(),
            delivery: NotificationDelivery::Received,
            tone,
            preview: NotificationPreview::Truncated,
            visibility: NotificationVisibility::Conversation,
            reference: Some(self.process_id.clone()),
            subtitle: Some(format!(
                "ran {}",
                crate::subagent::format_elapsed_secs(self.elapsed.as_secs())
            )),
            body,
            details: vec![
                format!("process: {}", self.process_id),
                format!("command: {}", self.command),
                format!("exit code: {}", exit_label(self.exit_code)),
            ],
        }
    }
}

/// Fence raw output so the card renderer keeps it verbatim instead of reading
/// it as markdown. The fence outgrows any backtick run inside the output.
fn fenced_output(output: &str) -> String {
    let longest_run = output
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    format!("{fence}text\n{output}\n{fence}")
}

pub(crate) fn notification_prompt(notifications: &[ProcessNotification]) -> String {
    let body_budget = MODEL_NOTIFICATION_BYTES
        .saturating_sub(NOTIFICATION_HEADER.len() + NOTIFICATION_FOOTER.len());
    let body = crate::tools::notification_format::join_budgeted_sections(
        notifications.iter().map(format_notification_summary),
        "\n\n",
        body_budget,
        |remaining| format!("... {remaining} process status section(s) omitted; use process poll"),
    );
    format!("{NOTIFICATION_HEADER}{body}{NOTIFICATION_FOOTER}")
}

pub(crate) fn excerpt_output(chunks: &[super::Chunk], budget: usize) -> String {
    let mut text = String::new();
    for chunk in chunks {
        text.push_str(&chunk.text);
        if text.len() > budget {
            break;
        }
    }
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

pub(crate) const fn output_excerpt_budget() -> usize {
    OUTPUT_EXCERPT_BYTES
}

fn format_notification_summary(notification: &ProcessNotification) -> String {
    let mut lines = vec![
        format!("process_id: {}", notification.process_id),
        format!("command: {}", notification.command),
        format!("state: {}", state_label(notification.state)),
        format!("exit_code: {}", exit_label(notification.exit_code)),
    ];
    if let Some(detail) = &notification.terminal_detail {
        lines.push(format!("detail: {detail}"));
    }
    if !notification.output.is_empty() {
        lines.push(String::new());
        lines.push(notification.output.clone());
    }
    lines.join("\n")
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Starting => "starting",
        State::Running => "running",
        State::Exited => "exited",
        State::Terminated => "terminated",
        State::TimedOut => "timed out",
        State::FailedToStart => "failed to start",
    }
}

fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(code) => code.to_string(),
        None => "none".into(),
    }
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
