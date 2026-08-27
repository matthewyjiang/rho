use std::{collections::BTreeMap, time::Duration};

use rho_sdk::{ModelCallMetrics, ModelCallProfile};

const MIN_GENERATION_OUTPUT_TOKENS: u64 = 32;
const MIN_GENERATION_TIME: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ModelPerformanceSummary {
    pub(super) latest_call: Option<ModelCallMetrics>,
    /// Token-weighted average of generation throughput (`tokens / generation_time`).
    pub(super) average_generation_tokens_per_second: Option<f64>,
    pub(super) eligible_calls: u64,
}

impl ModelPerformanceSummary {
    /// Average generation rate rounded for display (`N tok/s` in the
    /// statusline and the attach header).
    pub(super) fn rounded_generation_rate(&self) -> Option<u64> {
        self.average_generation_tokens_per_second
            .map(|rate| rate.round() as u64)
    }
}

#[derive(Default)]
pub(super) struct ModelPerformanceTracker {
    profiles: BTreeMap<ModelCallProfile, ModelPerformanceAggregate>,
}

impl ModelPerformanceTracker {
    pub(super) fn record(&mut self, profile: ModelCallProfile, metrics: ModelCallMetrics) {
        self.profiles.entry(profile).or_default().record(metrics);
    }

    pub(super) fn summary(&self, profile: &ModelCallProfile) -> ModelPerformanceSummary {
        self.profiles
            .get(profile)
            .map(ModelPerformanceAggregate::summary)
            .unwrap_or_default()
    }

    pub(super) fn clear(&mut self) {
        self.profiles.clear();
    }
}

#[derive(Default)]
pub(super) struct ModelPerformanceAggregate {
    latest_call: Option<ModelCallMetrics>,
    generation_output_tokens: u64,
    generation_time: Duration,
    eligible_calls: u64,
}

impl ModelPerformanceAggregate {
    fn record(&mut self, metrics: ModelCallMetrics) {
        self.latest_call = Some(metrics);
        let Some(generation_output_tokens) = metrics.resolved_generation_tokens() else {
            return;
        };
        let Some(generation_time) = metrics.generation_time else {
            return;
        };
        self.record_resolved(generation_output_tokens, generation_time);
    }

    /// Applies the same eligibility gates as [`Self::record`]: at least 32
    /// generated tokens and 500ms of generation time. Does not update
    /// `latest_call`.
    pub(super) fn record_resolved(
        &mut self,
        generation_output_tokens: u64,
        generation_time: Duration,
    ) {
        if generation_output_tokens < MIN_GENERATION_OUTPUT_TOKENS
            || generation_time < MIN_GENERATION_TIME
        {
            return;
        }

        self.generation_output_tokens = self
            .generation_output_tokens
            .saturating_add(generation_output_tokens);
        self.generation_time = self.generation_time.saturating_add(generation_time);
        self.eligible_calls = self.eligible_calls.saturating_add(1);
    }

    pub(super) fn summary(&self) -> ModelPerformanceSummary {
        ModelPerformanceSummary {
            latest_call: self.latest_call,
            average_generation_tokens_per_second: (self.eligible_calls > 0)
                .then(|| self.generation_output_tokens as f64 / self.generation_time.as_secs_f64()),
            eligible_calls: self.eligible_calls,
        }
    }
}

#[cfg(test)]
#[path = "model_performance_tests.rs"]
mod tests;
