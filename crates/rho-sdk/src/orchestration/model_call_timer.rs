use std::time::Instant;

use crate::{
    model::{GenerationOutputTokens, ModelEvent},
    ModelCallMetrics,
};

/// Burst-replay detection thresholds, measured against CLIProxyAPI
/// (claude-fable-5, 2026-08-30): honestly streamed calls arrived at ~35
/// tokens per generated delta with the generation window spanning ~89% of
/// total latency, while burst-replayed calls (the proxy flushing a buffered
/// response in 2-3 giant deltas) carried 158-238 tokens per delta with the
/// window spanning at most 11% of total latency. Direct providers stream far
/// fewer tokens per delta than either. Both gates must trip: chunky-but-live
/// streams (large deltas spread across the response) keep their rate.
const BURST_MIN_TOKENS_PER_GENERATED_EVENT: u64 = 64;
const BURST_MAX_WINDOW_FRACTION_OF_LATENCY: f64 = 0.5;

/// Times one model call. Every reported duration is scoped to the attempt that
/// produced the returned output: a discarded attempt and the backoff before the
/// retry belong to the retry policy, not to the model's speed.
pub(super) struct ModelCallTimer {
    attempt_started: Instant,
    first_generated: Option<Instant>,
    last_observed: Option<Instant>,
    generated_events: u64,
    generation_output_tokens: Option<GenerationOutputTokens>,
}

impl ModelCallTimer {
    pub(super) fn start(attempt_started: Instant) -> Self {
        Self {
            attempt_started,
            first_generated: None,
            last_observed: None,
            generated_events: 0,
            generation_output_tokens: None,
        }
    }

    /// Disowns a failed attempt and starts the clock over for the retry.
    pub(super) fn discard_attempt_output(&mut self, observed_at: Option<Instant>) {
        self.first_generated = None;
        self.last_observed = None;
        self.generated_events = 0;
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
        let generated = match event {
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
        };
        if generated {
            self.generated_events = self.generated_events.saturating_add(1);
            if self.first_generated.is_none() {
                self.first_generated = Some(observed_at);
            }
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
        let generation_time = self
            .first_generated
            .map(|first| stream_completed.duration_since(first));
        let total_latency = stream_completed.duration_since(self.attempt_started);
        let mut metrics = ModelCallMetrics {
            output_tokens,
            time_to_first_token: self
                .first_generated
                .map(|first| first.duration_since(self.attempt_started)),
            generation_time,
            total_latency,
            generation_output_tokens: self.generation_output_tokens,
        };
        if self.is_burst_replay(&metrics) {
            // The stream was buffered upstream (a translating proxy flushing a
            // held response in a few giant deltas), so the generation window
            // measures replay speed, not decode speed. Timing stays reported;
            // the tokens cannot be attributed to the window, so throughput
            // surfaces read this as unavailable rather than an inflated rate.
            metrics.generation_output_tokens = Some(GenerationOutputTokens::Unavailable);
        }
        metrics
    }

    /// True when the generated deltas were too few and too compressed for the
    /// generation window to describe model decode speed. Both gates must trip;
    /// see the threshold constants for the measurements behind them.
    fn is_burst_replay(&self, metrics: &ModelCallMetrics) -> bool {
        let Some(tokens) = metrics.resolved_generation_tokens() else {
            return false;
        };
        let Some(generation_time) = metrics.generation_time else {
            return false;
        };
        if self.generated_events == 0 {
            return false;
        }
        tokens / self.generated_events >= BURST_MIN_TOKENS_PER_GENERATED_EVENT
            && generation_time.as_secs_f64()
                <= BURST_MAX_WINDOW_FRACTION_OF_LATENCY * metrics.total_latency.as_secs_f64()
    }
}

#[cfg(test)]
#[path = "model_call_timer_tests.rs"]
mod tests;
