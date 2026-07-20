use rho_tools::tool::truncate;

use {super::agent::SubagentSnapshot, crate::subagent::RunState};

const DETAIL_BYTES: usize = 160;
const SUMMARY_DETAIL_BYTES: usize = 256;
const RESULT_EXCERPT_BYTES: usize = 16 * 1024;
pub(crate) const MODEL_NOTIFICATION_BYTES: usize = 16 * 1024;

const NOTIFICATION_HEADER: &str = "[agent notification]\n\nRun status:\n";
const NOTIFICATION_FOOTER: &str = "\n\nAny omitted or truncated result details remain available through `agents list`, `agents status`, or `rho attach <run-id>`.\n\nThis is an automated notification, not a user message. Fold the results into your ongoing work; use the agents tool for details.";
const NEWER_NOTIFICATIONS_SEPARATOR: &str = "\n\n--- newer agent completions ---\n\n";
const EARLIER_NOTIFICATIONS_TRUNCATED: &str =
    "\n[earlier agent notification context truncated; use `agents list` or `agents status`]";

#[derive(Clone, Copy)]
pub(super) enum SnapshotFormat {
    Completion,
    Status,
}

pub(super) fn format_background_start(id: &str, agent_id: &str) -> String {
    format!("agent {id} ({agent_id}) started in background\nattach: rho attach {id}")
}

pub(super) fn format_running(id: &str) -> String {
    format!("agent {id} running\nattach: rho attach {id}")
}

pub(super) fn format_snapshot(snapshot: &SubagentSnapshot, format: SnapshotFormat) -> String {
    let mut lines = vec![format!(
        "agent {} ({}): {}",
        snapshot.id,
        snapshot.agent_id,
        snapshot.status.state.as_str()
    )];
    let metrics = format!(
        "turns: {} · tokens: {} in / {} out",
        snapshot.status.turns, snapshot.status.input_tokens, snapshot.status.output_tokens
    );
    match format {
        SnapshotFormat::Completion => lines.push(metrics),
        SnapshotFormat::Status => {
            lines.push(format!(
                "elapsed: {} · {metrics}",
                format_elapsed(snapshot.elapsed.as_secs())
            ));
            // Progress metadata only: a running run's streamed text stays
            // private so the parent cannot act on results before delivery.
            if !snapshot.done {
                if let Some(activity) = &snapshot.status.last_activity {
                    lines.push(format!("activity: {activity}"));
                }
            }
        }
    }
    if matches!(format, SnapshotFormat::Completion) {
        if let Some(error) = &snapshot.status.error {
            lines.push(format!("error: {error}"));
        }
        if matches!(snapshot.status.state, RunState::Error | RunState::Stopped) {
            lines.push("this delegated task did not complete; treat its work as unverified".into());
        }
    }
    if matches!(format, SnapshotFormat::Completion) || !snapshot.done {
        if let Some(error) = &snapshot.status.attachment_error {
            lines.push(format!("attachment error: {error}"));
        }
    }
    if matches!(format, SnapshotFormat::Status) {
        lines.push(format!("attach: rho attach {}", snapshot.id));
    }
    if snapshot.done && matches!(format, SnapshotFormat::Completion) {
        if let Some(result) = snapshot
            .status
            .result
            .as_deref()
            .filter(|result| !result.is_empty())
        {
            lines.push(String::new());
            lines.push(truncate(result.to_string(), RESULT_EXCERPT_BYTES));
        }
    }
    lines.join("\n")
}

/// Formats one bounded model notification. Run summaries take priority over
/// result excerpts so a large result cannot hide the state of later runs.
pub(super) fn format_notification(snapshots: &[&SubagentSnapshot]) -> String {
    let body_bytes = MODEL_NOTIFICATION_BYTES
        .saturating_sub(NOTIFICATION_HEADER.len() + NOTIFICATION_FOOTER.len());
    let mut body = String::new();

    for (index, snapshot) in snapshots.iter().enumerate() {
        let separator = if index == 0 { "" } else { "\n" };
        let summary = completion_summary(snapshot).join("\n");
        let remaining_runs = snapshots.len() - index;
        let omission = format!(
            "{separator}... {remaining_runs} run status section(s) omitted; use `agents status`"
        );
        if body.len() + separator.len() + summary.len() + omission.len() > body_bytes {
            if body.len() + omission.len() <= body_bytes {
                body.push_str(&omission);
            }
            break;
        }
        body.push_str(separator);
        body.push_str(&summary);
    }

    let results = snapshots
        .iter()
        .filter_map(|snapshot| {
            snapshot
                .status
                .result
                .as_deref()
                .filter(|result| !result.is_empty())
                .map(|result| (*snapshot, result))
        })
        .collect::<Vec<_>>();
    if !results.is_empty() {
        let heading = "\n\nResult excerpts:";
        if body.len() + heading.len() > body_bytes {
            return format!("{NOTIFICATION_HEADER}{body}{NOTIFICATION_FOOTER}");
        }
        body.push_str(heading);
    }
    for (snapshot, result) in results {
        let label = format!("\n\nagent {}:\n", snapshot.id);
        if body.len() + label.len() >= body_bytes {
            break;
        }
        body.push_str(&label);
        let available = (body_bytes - body.len()).min(RESULT_EXCERPT_BYTES);
        push_excerpt(&mut body, result, available);
    }

    format!("{NOTIFICATION_HEADER}{body}{NOTIFICATION_FOOTER}")
}

pub(crate) fn merge_notification_context(existing: Option<&str>, newer: &str) -> String {
    debug_assert!(newer.len() <= MODEL_NOTIFICATION_BYTES);
    let Some(existing) = existing else {
        return newer.to_string();
    };
    if existing.len() + NEWER_NOTIFICATIONS_SEPARATOR.len() + newer.len()
        <= MODEL_NOTIFICATION_BYTES
    {
        return format!("{existing}{NEWER_NOTIFICATIONS_SEPARATOR}{newer}");
    }

    let reserved =
        NEWER_NOTIFICATIONS_SEPARATOR.len() + EARLIER_NOTIFICATIONS_TRUNCATED.len() + newer.len();
    let Some(prefix_bytes) = MODEL_NOTIFICATION_BYTES.checked_sub(reserved) else {
        return newer.to_string();
    };
    let boundary = previous_char_boundary(existing, prefix_bytes);
    format!(
        "{}{EARLIER_NOTIFICATIONS_TRUNCATED}{NEWER_NOTIFICATIONS_SEPARATOR}{newer}",
        &existing[..boundary]
    )
}

fn completion_summary(snapshot: &SubagentSnapshot) -> Vec<String> {
    let mut lines = vec![format!(
        "agent {} ({}): {}",
        snapshot.id,
        snapshot.agent_id,
        snapshot.status.state.as_str()
    )];
    lines.push(format!(
        "turns: {} · tokens: {} in / {} out",
        snapshot.status.turns, snapshot.status.input_tokens, snapshot.status.output_tokens
    ));
    if let Some(error) = &snapshot.status.error {
        lines.push(format!(
            "error: {}",
            truncate(error.clone(), SUMMARY_DETAIL_BYTES)
        ));
    }
    if matches!(snapshot.status.state, RunState::Error | RunState::Stopped) {
        lines.push("this delegated task did not complete; treat its work as unverified".into());
    }
    if let Some(error) = &snapshot.status.attachment_error {
        lines.push(format!(
            "attachment error: {}",
            truncate(error.clone(), SUMMARY_DETAIL_BYTES)
        ));
    }
    lines
}

fn previous_char_boundary(value: &str, max_bytes: usize) -> usize {
    let mut boundary = max_bytes.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn push_excerpt(output: &mut String, result: &str, max_bytes: usize) {
    if result.len() <= max_bytes {
        output.push_str(result);
        return;
    }

    const TRUNCATED: &str = "\n[truncated]";
    let prefix_bytes = max_bytes.saturating_sub(TRUNCATED.len());
    let boundary = previous_char_boundary(result, prefix_bytes);
    output.push_str(&result[..boundary]);
    if max_bytes >= TRUNCATED.len() {
        output.push_str(TRUNCATED);
    }
}

pub(super) fn format_list_entry(snapshot: &SubagentSnapshot) -> String {
    // Activity labels only: streamed text stays out of model-facing listings
    // so results are consumed through delivery, not previews.
    let detail = snapshot
        .status
        .last_activity
        .as_deref()
        .unwrap_or(if snapshot.done { "finished" } else { "working" });
    let detail = detail.lines().next().unwrap_or_default().trim();
    format!(
        "{}  {}  {}  {}  {}",
        snapshot.id,
        snapshot.agent_id,
        snapshot.status.state.as_str(),
        format_elapsed(snapshot.elapsed.as_secs()),
        truncate(detail.to_string(), DETAIL_BYTES)
    )
}

fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h {minutes:02}m")
}

#[cfg(test)]
#[path = "agent_output_tests.rs"]
mod tests;
