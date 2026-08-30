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

// Covers: a translating proxy that buffers a response and flushes it in a few
// giant deltas must not report an inflated generation rate; the compressed
// window cannot attribute tokens, so throughput reads as unavailable.
// Owner: SDK orchestration timing
#[test]
fn burst_replayed_stream_reports_throughput_unavailable() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    // Long silent wait, then two giant deltas close together (measured
    // CLIProxyAPI shape: ~230 tokens/delta, window ~8% of latency).
    timer.observe(
        &ModelEvent::OutputDelta("giant".into()),
        Some(started + Duration::from_millis(4900)),
    );
    timer.observe(
        &ModelEvent::OutputDelta("giant".into()),
        Some(started + Duration::from_millis(5300)),
    );

    let metrics = timer.finish(started + Duration::from_millis(5300), Some(460));

    assert_eq!(
        metrics.generation_output_tokens,
        Some(GenerationOutputTokens::Unavailable)
    );
    assert_eq!(metrics.generation_tokens_per_second(), None);
    // Timing itself stays honest for /info.
    assert_eq!(metrics.generation_time, Some(Duration::from_millis(400)));
    assert_eq!(metrics.total_latency, Duration::from_millis(5300));
}

// Covers: a live stream of many small deltas keeps its generation rate even
// when the total is large; only compressed replays trip the burst gate.
// Owner: SDK orchestration timing
#[test]
fn steadily_streamed_call_keeps_generation_tokens() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    for i in 0..17u64 {
        timer.observe(
            &ModelEvent::OutputDelta("delta".into()),
            Some(started + Duration::from_millis(1290 + i * 660)),
        );
    }

    let metrics = timer.finish(started + Duration::from_millis(11960), Some(602));

    assert_eq!(metrics.resolved_generation_tokens(), Some(602));
    assert!(metrics.generation_tokens_per_second().is_some());
}

// Covers: chunky deltas spread across the whole response are still a live
// stream; the window-fraction gate must hold the rate in place.
// Owner: SDK orchestration timing
#[test]
fn chunky_but_live_stream_keeps_generation_tokens() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    // Two large deltas, but the window spans most of the latency.
    timer.observe(
        &ModelEvent::OutputDelta("giant".into()),
        Some(started + Duration::from_millis(500)),
    );
    timer.observe(
        &ModelEvent::OutputDelta("giant".into()),
        Some(started + Duration::from_millis(5000)),
    );

    let metrics = timer.finish(started + Duration::from_millis(5000), Some(460));

    assert_eq!(metrics.resolved_generation_tokens(), Some(460));
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
