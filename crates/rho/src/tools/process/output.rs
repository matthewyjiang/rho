//! Compact model-facing text for the process tool.

use super::types::{Snapshot, State, Stream};

pub(super) fn format_snapshot(snapshot: &Snapshot) -> String {
    let mut lines = vec![
        format!("process_id: {}", snapshot.process_id),
        format!("command: {}", encode_header_value(&snapshot.command)),
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

/// Script-facing view of a snapshot (see [`process_output_schema`]).
///
/// Output is the same bounded chunk window the text shows; `next_cursor` is
/// what a later `poll` passes as `cursor`.
pub(super) fn structured_snapshot(snapshot: &Snapshot) -> serde_json::Value {
    let stream = |kind: Stream| {
        snapshot
            .chunks
            .iter()
            .filter(|chunk| chunk.stream == kind)
            .map(|chunk| chunk.text.as_str())
            .collect::<String>()
    };
    serde_json::json!({
        "process_id": snapshot.process_id,
        "command": snapshot.command,
        "state": snapshot.state.as_wire_str(),
        "exit_code": snapshot.exit_code,
        "stdout": stream(Stream::Stdout),
        "stderr": stream(Stream::Stderr),
        "next_cursor": snapshot.next_cursor,
        "truncated": snapshot.truncated,
    })
}

/// Script-facing result of `stop`: the request was accepted, not completed.
pub(super) fn structured_stop(process_id: &str) -> serde_json::Value {
    serde_json::json!({ "process_id": process_id, "state": "stop_requested" })
}

/// JSON Schema shared by every `process` action's structured content.
pub(crate) fn process_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "process_id": {"type": "string"},
            "command": {"type": "string"},
            "state": {
                "type": "string",
                "enum": ["starting", "running", "exited", "terminated", "timed_out",
                         "failed_to_start", "stop_requested"]
            },
            "exit_code": {"type": ["integer", "null"]},
            "stdout": {"type": "string"},
            "stderr": {"type": "string"},
            "next_cursor": {"type": "integer", "description": "Pass to poll as cursor"},
            "truncated": {"type": "boolean", "description": "Older output was dropped"}
        },
        "required": ["process_id", "state"]
    })
}

pub(super) fn format_stop(process_id: &str) -> String {
    format!("process_id: {process_id}\nstop requested")
}

/// Encode a snapshot header value so it cannot inject extra header lines.
pub(crate) fn encode_header_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            other => encoded.push(other),
        }
    }
    encoded
}

/// Inverse of [`encode_header_value`] for compact snapshot cards.
pub(crate) fn decode_header_value(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match chars.next() {
            Some('\\') => decoded.push('\\'),
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
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
