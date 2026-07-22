use super::goal::{format_elapsed_with, ElapsedPrecision};
use std::time::{Duration, Instant};

/// Tracks one reasoning stretch: timer + live Thinking... placeholder.
#[derive(Clone, Debug, Default)]
pub(super) struct ReasoningPhase {
    started_at: Option<Instant>,
    hidden_placeholder: bool,
}

impl ReasoningPhase {
    pub(super) fn begin_step(&mut self, show_reasoning: bool) {
        *self = Self {
            started_at: None,
            hidden_placeholder: !show_reasoning,
        };
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn on_reasoning_delta(&mut self, show_reasoning: bool) {
        if self.started_at.is_none() {
            self.started_at = Some(Instant::now());
        }
        self.hidden_placeholder = !show_reasoning;
    }

    /// Clears the placeholder. Returns elapsed when reasoning deltas were seen.
    pub(super) fn finalize(&mut self) -> Option<Duration> {
        self.hidden_placeholder = false;
        self.started_at
            .take()
            .map(|started_at| started_at.elapsed())
    }

    pub(super) fn hidden_placeholder(&self) -> bool {
        self.hidden_placeholder
    }

    pub(super) fn set_hidden_placeholder(&mut self, active: bool) {
        self.hidden_placeholder = active;
    }

    pub(super) fn has_started(&self) -> bool {
        self.started_at.is_some()
    }
}

/// Formats the post-reasoning summary line.
pub(super) fn thought_summary(elapsed: Duration) -> String {
    format!(
        "Thought for {}",
        format_elapsed_with(elapsed, ElapsedPrecision::TenthsUnderMinute)
    )
}

#[cfg(test)]
#[path = "reasoning_phase_tests.rs"]
mod tests;
