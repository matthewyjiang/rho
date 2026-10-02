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

/// Stop is an accepted request, not a fabricated process state.
#[derive(Clone, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(super) enum ProcessOutput {
    Snapshot {
        #[serde(flatten)]
        snapshot: Snapshot,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_budget: Option<OutputBudget>,
    },
    StopRequested {
        process_id: String,
    },
}

/// Script-visible accounting when JSON overhead requires a smaller output page.
#[derive(Clone, serde::Serialize, schemars::JsonSchema)]
pub(super) struct OutputBudget {
    max_output_bytes: usize,
    received_bytes: usize,
    deferred_chunks: usize,
    omitted_chunks: usize,
}

pub(super) fn render_snapshot(snapshot: Snapshot) -> rho_tools::Rendered<ProcessOutput> {
    let failed = match snapshot.state {
        State::Starting | State::Running => false,
        State::Exited => snapshot.exit_code != Some(0),
        State::Terminated | State::TimedOut | State::FailedToStart => true,
    };
    rho_tools::Rendered::new(
        format_snapshot(&snapshot),
        ProcessOutput::Snapshot {
            snapshot,
            output_budget: None,
        },
    )
    .failed_if(failed)
}

/// Page script data without changing model text. Poll cursors are stateless, so
/// a later poll can retrieve deferred chunks while they remain retained.
pub(super) fn limit_process_data(
    rendered: rho_tools::Rendered<ProcessOutput>,
    max_output_bytes: usize,
) -> Result<rho_tools::Rendered<ProcessOutput>, rho_sdk::tool::ToolError> {
    let Some(mut data) = rendered.data().cloned() else {
        return Ok(rendered);
    };
    let serialized_bytes = |data: &ProcessOutput| {
        serde_json::to_vec(data)
            .map(|bytes| bytes.len())
            .map_err(|error| {
                rho_sdk::tool::ToolError::new(
                    rho_sdk::tool::ToolErrorKind::Execution,
                    error.to_string(),
                )
            })
    };
    let received_bytes = serialized_bytes(&data)?;
    if received_bytes <= max_output_bytes {
        return Ok(rendered);
    }
    match &mut data {
        ProcessOutput::Snapshot { output_budget, .. } => {
            *output_budget = Some(OutputBudget {
                max_output_bytes,
                received_bytes,
                deferred_chunks: 0,
                omitted_chunks: 0,
            });
        }
        ProcessOutput::StopRequested { .. } => return Ok(rendered),
    }
    while serialized_bytes(&data)? > max_output_bytes {
        match &mut data {
            ProcessOutput::Snapshot {
                snapshot,
                output_budget,
            } => {
                let Some(chunk) = snapshot.chunks.pop() else {
                    // Pathological control-only metadata (such as a huge command)
                    // still falls through to the existing limit_data safety net.
                    break;
                };
                let budget = output_budget.as_mut().expect("snapshot budget initialized");
                if snapshot.chunks.is_empty() {
                    // Match poll_bounded: consume an indivisible oversized chunk
                    // rather than leave every subsequent poll stuck on it.
                    snapshot.next_cursor = chunk.cursor + 1;
                    snapshot.output_pending = snapshot.next_cursor < snapshot.available_cursor;
                    budget.omitted_chunks += 1;
                } else {
                    snapshot.next_cursor = chunk.cursor;
                    snapshot.output_pending = true;
                    budget.deferred_chunks += 1;
                }
            }
            ProcessOutput::StopRequested { .. } => unreachable!("snapshot budget initialized"),
        }
    }
    Ok(rho_tools::Rendered::new(rendered.text().to_owned(), data).failed_if(rendered.is_failure()))
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
        State::Exited | State::Terminated | State::TimedOut | State::FailedToStart => Some(code),
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
