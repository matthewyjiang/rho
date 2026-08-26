use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_sdk::{model::ServiceTier, ModelCallMetrics, ModelCallProfile, ReasoningLevel};

use super::{
    GenerationOutputTokens, ModelCallPerformance, ModelPerformanceSummary, ModelPerformanceTracker,
};

fn profile(
    provider: &str,
    model: &str,
    reasoning: ReasoningLevel,
    service_tier: Option<ServiceTier>,
) -> ModelCallProfile {
    ModelCallProfile {
        provider: provider.into(),
        model: model.into(),
        reasoning,
        service_tier,
    }
}

/// Builds metrics where generation time is the throughput window.
fn metrics(output_tokens: u64, generation_time: Duration) -> ModelCallMetrics {
    let time_to_first_token = Duration::from_millis(200);
    ModelCallMetrics {
        output_tokens: Some(output_tokens),
        time_to_first_token: Some(time_to_first_token),
        generation_time: Some(generation_time),
        total_latency: time_to_first_token + generation_time,
        generation_output_tokens: None,
    }
}

#[test]
fn computes_a_token_weighted_generation_average_from_completed_calls() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::Medium, None);
    tracker.record(
        profile.clone(),
        metrics(120, Duration::from_secs(2)),
        GenerationOutputTokens::Reported(100),
    );
    tracker.record(
        profile.clone(),
        metrics(330, Duration::from_secs(3)),
        GenerationOutputTokens::Reported(300),
    );

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(ModelCallPerformance {
                metrics: metrics(330, Duration::from_secs(3)),
                generation_output_tokens: GenerationOutputTokens::Reported(300),
            }),
            average_generation_tokens_per_second: Some(80.0),
            eligible_calls: 2,
        }
    );
}

#[test]
fn keeps_short_calls_as_latest_without_adding_them_to_the_average() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::Medium, None);
    let short_call = metrics(12, Duration::from_millis(300));

    tracker.record(
        profile.clone(),
        short_call,
        GenerationOutputTokens::Reported(12),
    );

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(ModelCallPerformance {
                metrics: short_call,
                generation_output_tokens: GenerationOutputTokens::Reported(12),
            }),
            average_generation_tokens_per_second: None,
            eligible_calls: 0,
        }
    );
}

// Covers: aggregate output remains the throughput fallback without a generation count.
// Owner: TUI model-performance aggregation
#[test]
fn falls_back_to_aggregate_output_without_generation_output() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::High, None);
    let aggregate_only = ModelCallMetrics {
        output_tokens: Some(100),
        time_to_first_token: Some(Duration::from_millis(200)),
        generation_time: Some(Duration::from_secs(2)),
        total_latency: Duration::from_millis(2_200),
        generation_output_tokens: None,
    };

    tracker.record(
        profile.clone(),
        aggregate_only,
        GenerationOutputTokens::AggregateFallback,
    );

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(ModelCallPerformance {
                metrics: aggregate_only,
                generation_output_tokens: GenerationOutputTokens::AggregateFallback,
            }),
            average_generation_tokens_per_second: Some(50.0),
            eligible_calls: 1,
        }
    );
}

// Covers: an invalid reasoning breakdown must not fall back to aggregate output.
// Owner: TUI model-performance aggregation
#[test]
fn unavailable_generation_output_suppresses_the_average() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::High, None);
    let invalid_breakdown = metrics(100, Duration::from_secs(2));

    tracker.record(
        profile.clone(),
        invalid_breakdown,
        GenerationOutputTokens::Unavailable,
    );

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(ModelCallPerformance {
                metrics: invalid_breakdown,
                generation_output_tokens: GenerationOutputTokens::Unavailable,
            }),
            average_generation_tokens_per_second: None,
            eligible_calls: 0,
        }
    );
}

// Covers: generation throughput needs a streamed generation interval, so a
// call with only hidden pre-stream work cannot enter the average.
// Owner: TUI model-performance aggregation
#[test]
fn ignores_calls_without_generation_time_for_the_average() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::High, None);
    let no_generation_window = ModelCallMetrics {
        output_tokens: Some(100),
        time_to_first_token: None,
        generation_time: None,
        total_latency: Duration::from_secs(2),
        generation_output_tokens: None,
    };

    tracker.record(
        profile.clone(),
        no_generation_window,
        GenerationOutputTokens::Reported(80),
    );

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(ModelCallPerformance {
                metrics: no_generation_window,
                generation_output_tokens: GenerationOutputTokens::Reported(80),
            }),
            average_generation_tokens_per_second: None,
            eligible_calls: 0,
        }
    );
}

#[test]
fn separates_model_profiles_including_service_tier() {
    let mut tracker = ModelPerformanceTracker::default();
    let standard = profile("openai", "model-a", ReasoningLevel::Medium, None);
    let priority = profile(
        "openai",
        "model-a",
        ReasoningLevel::Medium,
        Some(ServiceTier::Priority),
    );
    tracker.record(
        standard.clone(),
        metrics(120, Duration::from_secs(2)),
        GenerationOutputTokens::Reported(100),
    );
    tracker.record(
        priority.clone(),
        metrics(220, Duration::from_secs(2)),
        GenerationOutputTokens::Reported(200),
    );

    assert_eq!(
        tracker
            .summary(&standard)
            .average_generation_tokens_per_second,
        Some(50.0)
    );
    assert_eq!(
        tracker
            .summary(&priority)
            .average_generation_tokens_per_second,
        Some(100.0)
    );
    assert_eq!(
        tracker.summary(&profile("openai", "model-a", ReasoningLevel::High, None,)),
        ModelPerformanceSummary::default()
    );
    assert_eq!(
        tracker.summary(&profile(
            "anthropic",
            "model-a",
            ReasoningLevel::Medium,
            None,
        )),
        ModelPerformanceSummary::default()
    );
    assert_eq!(
        tracker.summary(&profile("openai", "model-b", ReasoningLevel::Medium, None,)),
        ModelPerformanceSummary::default()
    );
}
