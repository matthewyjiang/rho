use serde_json::Value;

use super::types::{Snapshot, State, Stream};

pub(super) fn result_lines(content: &str) -> Option<Vec<String>> {
    if let Ok(snapshot) = serde_json::from_str::<Snapshot>(content) {
        return Some(snapshot_lines(&snapshot));
    }

    let value: Value = serde_json::from_str(content).ok()?;
    let process_id = value.get("process_id")?.as_str()?;
    value
        .get("stop_requested")
        .and_then(Value::as_bool)
        .filter(|requested| *requested)
        .map(|_| vec!["process".into(), format!("stop requested: {process_id}")])
}

pub(super) fn snapshot_lines(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = vec!["process".into(), snapshot.command.clone()];
    lines.push(status_line(snapshot));

    if snapshot.truncated {
        lines.push(format!(
            "output before cursor {} is no longer available",
            snapshot.first_cursor
        ));
    }
    for chunk in &snapshot.chunks {
        let label = match chunk.stream {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        };
        lines.push(format!("{label}:"));
        lines.push(chunk.text.clone());
    }
    if snapshot.output_pending {
        lines.push(format!(
            "more output available at cursor {}",
            snapshot.next_cursor
        ));
    }
    if let Some(detail) = &snapshot.terminal_detail {
        lines.push(format!("detail: {detail}"));
    }
    lines
}

fn status_line(snapshot: &Snapshot) -> String {
    let state = match snapshot.state {
        State::Starting => "starting",
        State::Running => "running",
        State::Exited => "exited",
        State::Terminated => "terminated",
        State::TimedOut => "timed out",
        State::FailedToStart => "failed to start",
    };
    let exit_code = snapshot
        .exit_code
        .map(|code| format!(", exit code {code}"))
        .unwrap_or_default();
    format!(
        "{state} - {} - {:.1}s{exit_code}",
        snapshot.process_id, snapshot.runtime_seconds
    )
}
