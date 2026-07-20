use rho_tools::tool::truncate;

use {super::agent::SubagentSnapshot, crate::subagent::RunState};

const DETAIL_BYTES: usize = 160;
const RESULT_EXCERPT_BYTES: usize = 16 * 1024;

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
