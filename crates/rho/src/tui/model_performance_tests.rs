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

/// Builds metrics for a call that succeeded on its first attempt, where the
/// attempt spans the whole request and drives the reported rate.
fn metrics(output_tokens: u64, attempt_latency: Duration) -> ModelCallMetrics {
    ModelCallMetrics {
        output_tokens: Some(output_tokens),
        time_to_first_token: Some(Duration::from_millis(200)),
        generation_time: attempt_latency.checked_sub(Duration::from_millis(200)),
        attempt_latency,
        total_latency: attempt_latency,
    }
}

#[test]
fn computes_a_token_weighted_average_from_completed_calls() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::Medium, None);
    tracker.record(profile.clone(), metrics(100, Duration::from_secs(2)));
    tracker.record(profile.clone(), metrics(300, Duration::from_secs(3)));

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(metrics(300, Duration::from_secs(3))),
            average_output_tokens_per_second: Some(80.0),
            eligible_calls: 2,
        }
    );
}

#[test]
fn keeps_short_calls_as_latest_without_adding_them_to_the_average() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::Medium, None);
    let short_call = metrics(12, Duration::from_millis(300));

    tracker.record(profile.clone(), short_call);

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(short_call),
            average_output_tokens_per_second: None,
            eligible_calls: 0,
        }
    );
}

// Covers: eligibility no longer depends on stream timing, so a call whose output
// was all hidden reasoning still contributes to the average.
// Owner: tui model-performance aggregation
#[test]
fn counts_calls_that_reported_tokens_without_streaming_any_output() {
    let mut tracker = ModelPerformanceTracker::default();
    let profile = profile("openai", "model", ReasoningLevel::High, None);
    let reasoning_only = ModelCallMetrics {
        output_tokens: Some(100),
        time_to_first_token: None,
        generation_time: None,
        attempt_latency: Duration::from_secs(2),
        total_latency: Duration::from_secs(2),
    };

    tracker.record(profile.clone(), reasoning_only);

    assert_eq!(
        tracker.summary(&profile),
        ModelPerformanceSummary {
            latest_call: Some(reasoning_only),
            average_output_tokens_per_second: Some(50.0),
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
    tracker.record(standard.clone(), metrics(100, Duration::from_secs(2)));
    tracker.record(priority.clone(), metrics(200, Duration::from_secs(2)));

    assert_eq!(
        tracker.summary(&standard).average_output_tokens_per_second,
        Some(50.0)
    );
    assert_eq!(
        tracker.summary(&priority).average_output_tokens_per_second,
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
