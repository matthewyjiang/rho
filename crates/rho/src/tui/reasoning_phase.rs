use super::goal::duration_summary;
use std::time::{Duration, Instant};

/// Tracks one reasoning stretch: open window + timer for Thought for … summaries.
///
/// Display policy (full text vs Thinking... vs hidden) lives on
/// [`super::ReasoningChrome`], not here.
#[derive(Clone, Debug, Default)]
pub(super) struct ReasoningPhase {
    /// True between [`Self::begin_step`] and [`Self::finalize`] / [`Self::reset`].
    open: bool,
    started_at: Option<Instant>,
}

impl ReasoningPhase {
    /// Open a new reasoning stretch for the current provider step.
    pub(super) fn begin_step(&mut self) {
        *self = Self {
            open: true,
            started_at: None,
        };
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn on_reasoning_delta(&mut self) {
        if self.started_at.is_none() {
            self.started_at = Some(Instant::now());
        }
    }

    /// Closes the stretch. Returns elapsed when reasoning deltas were seen.
    pub(super) fn finalize(&mut self) -> Option<Duration> {
        self.open = false;
        self.started_at
            .take()
            .map(|started_at| started_at.elapsed())
    }

    /// Whether the current step's reasoning stretch is still open.
    pub(super) fn is_open(&self) -> bool {
        self.open
    }
}

/// Formats the post-reasoning summary line.
pub(super) fn thought_summary(elapsed: Duration) -> String {
    duration_summary("Thought for", elapsed)
}

/// Formats the post-turn duration receipt on the assistant entry.
pub(super) fn worked_summary(elapsed: Duration) -> String {
    duration_summary("Worked for", elapsed)
}

#[cfg(test)]
#[path = "reasoning_phase_tests.rs"]
mod tests;
