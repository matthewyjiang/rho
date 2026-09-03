//! Decide whether a completed Claude process produced a usable result.

use super::stream::{TerminalClassification, TerminalResult};

/// The protocol and process result after Claude has exited.
pub(crate) enum TerminalOutcome {
    Success(TerminalResult),
    Failure {
        terminal: Option<TerminalResult>,
        detail: String,
        /// Prefer `detail` over a stream result/error message when rendering.
        prefer_detail: bool,
    },
}

/// Combine Claude's terminal message with its exit status.
///
/// An explicit valid success and exit code zero are both required. Callers own
/// their persistence and presentation policy, but share this protocol truth.
///
/// Non-zero exit often still carries the real reason on the stream-json
/// `result` line (API/safeguard errors, empty stderr). Prefer that text over a
/// bare exit code so advisor tools and subagent cards surface it.
pub(crate) fn assess_terminal(
    pending: Option<TerminalResult>,
    exit_status: std::process::ExitStatus,
    stderr: &str,
    program_label: &'static str,
) -> TerminalOutcome {
    if !exit_status.success() {
        let detail = non_zero_exit_detail(pending.as_ref(), exit_status, stderr, program_label);
        return TerminalOutcome::Failure {
            terminal: pending,
            detail,
            prefer_detail: true,
        };
    }

    match pending {
        Some(
            terminal @ TerminalResult {
                classification: TerminalClassification::Success { .. },
                ..
            },
        ) => TerminalOutcome::Success(terminal),
        Some(
            terminal @ TerminalResult {
                classification:
                    TerminalClassification::Failure { .. } | TerminalClassification::Invalid { .. },
                ..
            },
        ) => {
            let detail = terminal
                .error
                .clone()
                .or_else(|| terminal.result_text.clone())
                .unwrap_or_else(|| format!("{program_label}: terminal result was not success"));
            TerminalOutcome::Failure {
                terminal: Some(terminal),
                detail,
                prefer_detail: false,
            }
        }
        None => TerminalOutcome::Failure {
            terminal: None,
            detail: format!("{program_label}: stream ended without a terminal result message"),
            prefer_detail: true,
        },
    }
}

/// Failure text when the Claude process exits uncleanly.
///
/// Order of preference:
/// 1. stream-json failure/invalid text (safeguards, API errors) when stderr is empty
/// 2. exit status plus stderr, optionally followed by stream failure text
fn non_zero_exit_detail(
    pending: Option<&TerminalResult>,
    exit_status: std::process::ExitStatus,
    stderr: &str,
    program_label: &'static str,
) -> String {
    let process_detail = if stderr.is_empty() {
        format!("{program_label}: process exited with {exit_status}")
    } else {
        format!("{program_label}: process exited with {exit_status}: {stderr}")
    };

    let Some(stream_error) = stream_failure_text(pending) else {
        return process_detail;
    };

    // Protocol failures (subtype success + is_error, or error_*) often exit 1
    // with an empty stderr and put the human-readable reason only on the
    // result line. That line is the message the caller needs.
    if stderr.is_empty() {
        stream_error
    } else {
        format!("{process_detail}\n{stream_error}")
    }
}

/// Error text from a failed or invalid terminal result, if any.
///
/// Success terminals are ignored here: a success stream with a non-zero exit
/// is a process failure, not a stream-reported failure, so the answer text
/// must not replace the exit diagnosis.
fn stream_failure_text(pending: Option<&TerminalResult>) -> Option<String> {
    let terminal = pending?;
    match &terminal.classification {
        TerminalClassification::Success { .. } => None,
        TerminalClassification::Failure { .. } | TerminalClassification::Invalid { .. } => terminal
            .error
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                terminal
                    .result_text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            }),
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
