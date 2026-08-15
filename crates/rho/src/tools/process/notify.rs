//! Parent-session delivery for finished background processes.
//!
//! Mirrors workflow and delegated-agent notifications: the manager stores a
//! terminal snapshot, and the TUI drains unobserved terminals at the next
//! turn boundary.

use super::types::State;

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
}

pub(crate) fn notification_prompts(notifications: &[ProcessNotification]) -> (String, String) {
    let body_budget = MODEL_NOTIFICATION_BYTES
        .saturating_sub(NOTIFICATION_HEADER.len() + NOTIFICATION_FOOTER.len());
    let mut body = String::new();
    for (index, notification) in notifications.iter().enumerate() {
        let separator = if index == 0 { "" } else { "\n\n" };
        let summary = format_notification_summary(notification);
        if body.len() + separator.len() + summary.len() > body_budget {
            let remaining = notifications.len() - index;
            let omission = format!(
                "{separator}... {remaining} process status section(s) omitted; use process poll"
            );
            if body.len() + omission.len() <= body_budget {
                body.push_str(&omission);
            }
            break;
        }
        body.push_str(separator);
        body.push_str(&summary);
    }
    let model = format!("{NOTIFICATION_HEADER}{body}{NOTIFICATION_FOOTER}");
    let display = notifications
        .iter()
        .map(|notification| {
            format!(
                "process {} {} - {}",
                notification.process_id,
                state_label(notification.state),
                exit_label(notification.exit_code)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    (model, display)
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
