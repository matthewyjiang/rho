use std::{collections::BTreeMap, time::Duration};

use rho_sdk::{ModelCallMetrics, ModelCallProfile};

const MIN_OUTPUT_TOKENS: u64 = 32;
const MIN_ATTEMPT_LATENCY: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ModelPerformanceSummary {
    pub(super) latest_call: Option<ModelCallMetrics>,
    pub(super) average_output_tokens_per_second: Option<f64>,
    pub(super) eligible_calls: u64,
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
struct ModelPerformanceAggregate {
    latest_call: Option<ModelCallMetrics>,
    output_tokens: u64,
    attempt_latency: Duration,
    eligible_calls: u64,
}

impl ModelPerformanceAggregate {
    fn record(&mut self, metrics: ModelCallMetrics) {
        self.latest_call = Some(metrics);
        let Some(output_tokens) = metrics.output_tokens else {
            return;
        };
        if output_tokens < MIN_OUTPUT_TOKENS || metrics.attempt_latency < MIN_ATTEMPT_LATENCY {
            return;
        }

        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.attempt_latency = self.attempt_latency.saturating_add(metrics.attempt_latency);
        self.eligible_calls = self.eligible_calls.saturating_add(1);
    }

    fn summary(&self) -> ModelPerformanceSummary {
        ModelPerformanceSummary {
            latest_call: self.latest_call,
            average_output_tokens_per_second: (self.eligible_calls > 0)
                .then(|| self.output_tokens as f64 / self.attempt_latency.as_secs_f64()),
            eligible_calls: self.eligible_calls,
        }
    }
}

#[cfg(test)]
#[path = "model_performance_tests.rs"]
mod tests;
