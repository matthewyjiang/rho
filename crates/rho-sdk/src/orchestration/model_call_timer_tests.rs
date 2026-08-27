use std::time::{Duration, Instant};

use crate::model::{GenerationOutputTokens, ModelEvent, ModelUsage};

use super::ModelCallTimer;

#[test]
fn records_first_generated_and_final_provider_observations() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    let first_output = started + Duration::from_secs(2);
    let stream_completed = started + Duration::from_secs(3);

    timer.observe(&ModelEvent::OutputDelta("done".into()), Some(first_output));
    timer.observe(
        &ModelEvent::Usage(ModelUsage::default()),
        Some(stream_completed),
    );

    let metrics = timer.finish(started + Duration::from_secs(8), Some(4));
    assert_eq!(metrics.time_to_first_token, Some(Duration::from_secs(2)));
    assert_eq!(metrics.generation_time, Some(Duration::from_secs(1)));
    assert_eq!(metrics.total_latency, Duration::from_secs(3));
}

// Covers: a provider reasoning breakdown must stay separate from aggregate
// output usage until the host chooses its performance numerator.
// Owner: SDK orchestration timing
#[test]
fn generation_output_tokens_stay_separate_from_aggregate_metrics() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    timer.observe(
        &ModelEvent::OutputDelta("done".into()),
        Some(started + Duration::from_secs(1)),
    );
    timer.observe_generation_output_tokens(GenerationOutputTokens::Reported(30));

    let metrics = timer.finish(started + Duration::from_secs(3), Some(100));

    assert_eq!(metrics.output_tokens, Some(100));
    assert_eq!(
        metrics.generation_output_tokens,
        Some(GenerationOutputTokens::Reported(30))
    );
}

// Covers: a discarded attempt and the backoff before the retry must not be
// charged to the attempt that produced the returned output.
// Owner: sdk orchestration
#[test]
fn failed_attempt_restarts_every_duration_at_the_retry() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    timer.observe(
        &ModelEvent::OutputDelta("failed".into()),
        Some(started + Duration::from_secs(1)),
    );
    timer.observe_generation_output_tokens(GenerationOutputTokens::Reported(99));

    let retry_started = started + Duration::from_secs(3);
    timer.discard_attempt_output(Some(retry_started));
    let final_first_output = started + Duration::from_secs(4);
    timer.observe(
        &ModelEvent::OutputDelta("done".into()),
        Some(final_first_output),
    );

    let metrics = timer.finish(started + Duration::from_secs(5), Some(4));
    // The first attempt ran for 1s and the backoff lasted 2s. Neither is
    // charged to the retry, which produced its first event 1s after starting.
    assert_eq!(metrics.time_to_first_token, Some(Duration::from_secs(1)));
    assert_eq!(metrics.total_latency, Duration::from_secs(1));
    assert_eq!(metrics.output_tokens, Some(4));
    assert_eq!(metrics.generation_output_tokens, None);
}

#[test]
fn synthesized_output_has_no_generation_timing() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    timer.observe(&ModelEvent::OutputDelta("done".into()), None);

    let metrics = timer.finish(started + Duration::from_secs(2), Some(4));
    assert_eq!(metrics.time_to_first_token, None);
    assert_eq!(metrics.generation_time, None);
    assert_eq!(metrics.total_latency, Duration::from_secs(2));
}

#[test]
fn tool_call_delta_counts_as_generated_output() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    let first_output = started + Duration::from_millis(400);

    timer.observe(
        &ModelEvent::ToolCallDelta {
            index: 0,
            id: Some("call-1".into()),
            name: Some("search".into()),
            arguments: "{\"query\":\"example\"}".into(),
        },
        Some(first_output),
    );

    let metrics = timer.finish(first_output, Some(8));
    assert_eq!(
        metrics.time_to_first_token,
        Some(Duration::from_millis(400))
    );
}
