//! Decide whether a completed Claude process produced a usable result.

use super::{spawn, stream::TerminalResult};

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
pub(crate) fn assess_terminal(
    pending: Option<TerminalResult>,
    exit_status: std::process::ExitStatus,
    stderr: &str,
) -> TerminalOutcome {
    if !exit_status.success() {
        let detail = if spawn::looks_like_max_turns_unsupported(stderr) {
            "claude code: this claude binary rejected --max-turns; upgrade Claude Code or remove the turn cap".into()
        } else if stderr.is_empty() {
            format!("claude code: process exited with {exit_status}")
        } else {
            format!("claude code: process exited with {exit_status}: {stderr}")
        };
        return TerminalOutcome::Failure {
            terminal: pending,
            detail,
            prefer_detail: true,
        };
    }

    match pending {
        Some(terminal) if terminal.classification.is_success() => TerminalOutcome::Success(terminal),
        Some(terminal)
            if terminal.classification.is_failure() || terminal.classification.is_invalid() =>
        {
            let detail = terminal
                .error
                .clone()
                .or_else(|| terminal.result_text.clone())
                .unwrap_or_else(|| "claude code: terminal result was not success".into());
            TerminalOutcome::Failure {
                terminal: Some(terminal),
                detail,
                prefer_detail: false,
            }
        }
        Some(terminal) => TerminalOutcome::Failure {
            terminal: Some(terminal),
            detail: "claude code: terminal result classification was not success".into(),
            prefer_detail: true,
        },
        None => TerminalOutcome::Failure {
            terminal: None,
            detail: "claude code: stream ended without a terminal result message; see log.txt for details"
                .into(),
            prefer_detail: true,
        },
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
