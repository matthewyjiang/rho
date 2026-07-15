use super::types::{Snapshot, State, Stream};

/// Progress lines rendered beneath the presenter-owned tool header.
pub(super) fn snapshot_progress_lines(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = vec![status_line(snapshot)];

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
