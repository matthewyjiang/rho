use std::time::{Duration, Instant};

use crate::model::{ModelEvent, ModelUsage};

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

#[test]
fn failed_attempt_discards_output_without_moving_request_start() {
    let started = Instant::now();
    let mut timer = ModelCallTimer::start(started);
    timer.observe(
        &ModelEvent::OutputDelta("failed".into()),
        Some(started + Duration::from_secs(1)),
    );

    timer.discard_attempt_output();
    let final_first_output = started + Duration::from_secs(4);
    timer.observe(
        &ModelEvent::OutputDelta("done".into()),
        Some(final_first_output),
    );

    let metrics = timer.finish(started + Duration::from_secs(5), Some(4));
    assert_eq!(metrics.time_to_first_token, Some(Duration::from_secs(4)));
    assert_eq!(metrics.total_latency, Duration::from_secs(4));
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
