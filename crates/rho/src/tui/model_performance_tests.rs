use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_sdk::{model::ServiceTier, ModelCallMetrics, ModelCallProfile, ReasoningLevel};

use super::{ModelPerformanceSummary, ModelPerformanceTracker};

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
fn metrics(
    output_tokens: u64,
    generation_time: Duration,
    generation_output_tokens: Option<rho_sdk::model::GenerationOutputTokens>,
) -> ModelCallMetrics {
    let time_to_first_token = Duration::from_millis(200);
    ModelCallMetrics {
        output_tokens: Some(output_tokens),
        time_to_first_token: Some(time_to_first_token),
        generation_time: Some(generation_time),
        total_latency: time_to_first_token + generation_time,
        generation_output_tokens,
    }
}

#[test]
fn computes_a_token_weighted_generation_average_from_completed_calls() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::Medium, None);
    tracker.record(
        profile.clone(),
        metrics(
            120,
            Duration::from_secs(2),
            Some(rho_sdk::model::GenerationOutputTokens::Reported(100)),
        ),
    );
    tracker.record(
        profile.clone(),
        metrics(
            330,
            Duration::from_secs(3),
            Some(rho_sdk::model::GenerationOutputTokens::Reported(300)),
        ),
    );

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(metrics(
                330,
                Duration::from_secs(3),
                Some(rho_sdk::model::GenerationOutputTokens::Reported(300)),
            )),
            average_generation_tokens_per_second: Some(80.0),
            eligible_calls: 2,
        }
    );
}

// Covers: calls below either eligibility gate (32 generated tokens, 500ms of
// generation), with an invalid reasoning breakdown, or without a streamed
// generation interval stay the latest call but never enter the average.
// Owner: TUI model-performance aggregation
#[test]
fn ineligible_calls_stay_latest_without_entering_the_average() {
    let reported = |tokens| Some(rho_sdk::model::GenerationOutputTokens::Reported(tokens));
    let cases = [
        (
            "too few tokens",
            metrics(12, Duration::from_secs(2), reported(12)),
        ),
        (
            "too short generation",
            metrics(100, Duration::from_millis(300), reported(100)),
        ),
        (
            "invalid reasoning breakdown",
            metrics(
                100,
                Duration::from_secs(2),
                Some(rho_sdk::model::GenerationOutputTokens::Unavailable),
            ),
        ),
        (
            "no generation window",
            ModelCallMetrics {
                output_tokens: Some(100),
                time_to_first_token: None,
                generation_time: None,
                total_latency: Duration::from_secs(2),
                generation_output_tokens: reported(80),
            },
        ),
    ];
    for (case, call) in cases {
        let mut tracker = ModelPerformanceTracker::default();
        let profile = profile("openai", "model", ReasoningLevel::High, None);

        tracker.record(profile.clone(), call);

        assert_eq!(
            tracker.summary(&profile),
            ModelPerformanceSummary {
                latest_call: Some(call),
                average_generation_tokens_per_second: None,
                eligible_calls: 0,
            },
            "{case}"
        );
    }
}

// Covers: aggregate output remains the throughput fallback without a generation count.
// Owner: TUI model-performance aggregation
#[test]
fn falls_back_to_aggregate_output_without_generation_output() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::High, None);
    let aggregate_only = metrics(100, Duration::from_secs(2), None);

    tracker.record(profile.clone(), aggregate_only);

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(aggregate_only),
            average_generation_tokens_per_second: Some(50.0),
            eligible_calls: 1,
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
        metrics(
            120,
            Duration::from_secs(2),
            Some(rho_sdk::model::GenerationOutputTokens::Reported(100)),
        ),
    );
    tracker.record(
        priority.clone(),
        metrics(
            220,
            Duration::from_secs(2),
            Some(rho_sdk::model::GenerationOutputTokens::Reported(200)),
        ),
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
