//! Compact model-facing text for the process tool.

use super::types::{Snapshot, State, Stream};

pub(super) fn format_snapshot(snapshot: &Snapshot) -> String {
    let mut lines = vec![
        format!("process_id: {}", snapshot.process_id),
        format!("command: {}", snapshot.command),
        format!("state: {}", snapshot.state.as_wire_str()),
        format!("next: {}", snapshot.next_cursor),
    ];
    if snapshot.truncated {
        lines.push(format!("truncated: first={}", snapshot.first_cursor));
    }
    if snapshot.output_pending {
        lines.push("pending".into());
    }
    if let Some(code) = failure_exit_code(snapshot) {
        lines.push(format!("exit: {code}"));
    }
    if let Some(detail) = &snapshot.terminal_detail {
        lines.push(format!("detail: {detail}"));
    }
    let header_len = lines.len();
    push_stream(&mut lines, "stdout", snapshot, Stream::Stdout);
    push_stream(&mut lines, "stderr", snapshot, Stream::Stderr);
    if lines.len() > header_len {
        lines.insert(header_len, String::new());
    }
    lines.join("\n")
}

pub(super) fn format_stop(process_id: &str) -> String {
    format!("process_id: {process_id}\nstop requested")
}

fn failure_exit_code(snapshot: &Snapshot) -> Option<i32> {
    let code = snapshot.exit_code?;
    match snapshot.state {
        State::Starting | State::Running => None,
        State::Exited if code == 0 => None,
        _ => Some(code),
    }
}

fn push_stream(lines: &mut Vec<String>, label: &str, snapshot: &Snapshot, stream: Stream) {
    let mut body = String::new();
    for chunk in &snapshot.chunks {
        if chunk.stream == stream {
            body.push_str(&chunk.text);
        }
    }
    if body.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    lines.push(body);
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
