//! Elapsed-time text for the activity row and the transcript's receipts.

use std::time::Duration;

/// Tenths under one minute (`9.0s`), whole units from there on.
pub(super) fn format_elapsed(elapsed: Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("{:.1}s", elapsed.as_secs_f64())
    } else {
        crate::subagent::format_elapsed_secs(elapsed.as_secs())
    }
}

/// Formats the post-reasoning summary line (`Thought for 1.5s`).
pub(super) fn thought_summary(elapsed: Duration) -> String {
    format!("Thought for {}", format_elapsed(elapsed))
}

/// Formats the post-turn duration receipt on the assistant entry.
pub(super) fn worked_summary(elapsed: Duration) -> String {
    format!("Worked for {}", format_elapsed(elapsed))
}
