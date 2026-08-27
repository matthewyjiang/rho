use std::time::Instant;

use crate::{
    model::{GenerationOutputTokens, ModelEvent},
    ModelCallMetrics,
};

/// Times one model call. Every reported duration is scoped to the attempt that
/// produced the returned output: a discarded attempt and the backoff before the
/// retry belong to the retry policy, not to the model's speed.
pub(super) struct ModelCallTimer {
    attempt_started: Instant,
    first_generated: Option<Instant>,
    last_observed: Option<Instant>,
    generation_output_tokens: Option<GenerationOutputTokens>,
}

impl ModelCallTimer {
    pub(super) fn start(attempt_started: Instant) -> Self {
        Self {
            attempt_started,
            first_generated: None,
            last_observed: None,
            generation_output_tokens: None,
        }
    }

    /// Disowns a failed attempt and starts the clock over for the retry.
    pub(super) fn discard_attempt_output(&mut self, observed_at: Option<Instant>) {
        self.first_generated = None;
        self.last_observed = None;
        self.generation_output_tokens = None;
        if let Some(observed_at) = observed_at {
            self.attempt_started = observed_at;
        }
    }

    pub(super) fn observe(&mut self, event: &ModelEvent, observed_at: Option<Instant>) {
        let Some(observed_at) = observed_at else {
            return;
        };
        self.last_observed = Some(observed_at);
        if self.first_generated.is_none()
            && match event {
                ModelEvent::OutputDelta(text)
                | ModelEvent::ReasoningDelta(text)
                | ModelEvent::ReasoningSummaryDelta(text) => !text.is_empty(),
                ModelEvent::ToolCallDelta { .. } => true,
                ModelEvent::WebSearch(_)
                | ModelEvent::ProviderContext { .. }
                | ModelEvent::Usage(_)
                | ModelEvent::GenerationOutputTokens(_)
                | ModelEvent::HostedToolActivity { .. }
                | ModelEvent::ServiceTierFallback { .. } => false,
            }
        {
            self.first_generated = Some(observed_at);
        }
    }

    pub(super) fn observe_generation_output_tokens(&mut self, tokens: GenerationOutputTokens) {
        self.generation_output_tokens = Some(tokens);
    }

    pub(super) fn finish(
        &self,
        completed: Instant,
        output_tokens: Option<u64>,
    ) -> ModelCallMetrics {
        // A provider future may return only after queued events pass through a
        // backpressured host channel. The final provider observation marks the
        // stream boundary without charging that host delay to the model.
        let stream_completed = self.last_observed.unwrap_or(completed);
        ModelCallMetrics {
            output_tokens,
            time_to_first_token: self
                .first_generated
                .map(|first| first.duration_since(self.attempt_started)),
            generation_time: self
                .first_generated
                .map(|first| stream_completed.duration_since(first)),
            total_latency: stream_completed.duration_since(self.attempt_started),
            generation_output_tokens: self.generation_output_tokens,
        }
    }
}

#[cfg(test)]
#[path = "model_call_timer_tests.rs"]
mod tests;
