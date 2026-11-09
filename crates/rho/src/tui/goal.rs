use std::time::Duration;

pub(super) use crate::goal::{
    evaluate, EvaluationRequest, GoalDisposition, GoalLoopState, GoalState, EVALUATION_TIMEOUT,
    MAX_GOAL_CHARS,
};
#[cfg(test)]
pub(super) use crate::goal::{GoalEvaluation, HumanStep};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ElapsedPrecision {
    /// Whole seconds under one minute (`9s`).
    WholeSeconds,
    /// Tenths under one minute (`9.0s`), used by thought and turn receipts.
    TenthsUnderMinute,
}

pub(super) fn format_elapsed(elapsed: Duration) -> String {
    format_elapsed_with(elapsed, ElapsedPrecision::WholeSeconds)
}

pub(super) fn format_elapsed_with(elapsed: Duration, precision: ElapsedPrecision) -> String {
    match precision {
        ElapsedPrecision::WholeSeconds => crate::subagent::format_elapsed_secs(elapsed.as_secs()),
        ElapsedPrecision::TenthsUnderMinute => {
            if elapsed.as_secs() < 60 {
                format!("{:.1}s", elapsed.as_secs_f64())
            } else {
                crate::subagent::format_elapsed_secs(elapsed.as_secs())
            }
        }
    }
}

/// Formats a dim duration receipt (`Thought for 1.5s`, `Worked for 15.0s`).
pub(super) fn duration_summary(prefix: &str, elapsed: Duration) -> String {
    format!(
        "{prefix} {}",
        format_elapsed_with(elapsed, ElapsedPrecision::TenthsUnderMinute)
    )
}

/// Formats the post-reasoning summary line.
pub(super) fn thought_summary(elapsed: Duration) -> String {
    duration_summary("Thought for", elapsed)
}

/// Formats the post-turn duration receipt on the assistant entry.
pub(super) fn worked_summary(elapsed: Duration) -> String {
    duration_summary("Worked for", elapsed)
}
