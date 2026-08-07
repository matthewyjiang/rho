use std::time::Duration;

use pretty_assertions::assert_eq;

use super::ModelCallMetrics;

// Covers: end-to-end rate keeps pre-stream time in the denominator.
// Owner: SDK model-call metrics
#[test]
fn output_rate_divides_tokens_by_total_latency() {
    let cases = [
        (
            "reasoning before the first event is charged to the rate",
            ModelCallMetrics {
                output_tokens: Some(100),
                time_to_first_token: Some(Duration::from_secs(8)),
                generation_time: Some(Duration::from_secs(2)),
                total_latency: Duration::from_secs(10),
            },
            Some(10.0),
        ),
        (
            "a call that never streamed still reports a rate",
            ModelCallMetrics {
                output_tokens: Some(50),
                time_to_first_token: None,
                generation_time: None,
                total_latency: Duration::from_secs(5),
            },
            Some(10.0),
        ),
        (
            "no reported output tokens means no rate",
            ModelCallMetrics {
                output_tokens: None,
                time_to_first_token: None,
                generation_time: None,
                total_latency: Duration::from_secs(5),
            },
            None,
        ),
        (
            "a zero-length attempt means no rate",
            ModelCallMetrics {
                output_tokens: Some(50),
                time_to_first_token: None,
                generation_time: None,
                total_latency: Duration::ZERO,
            },
            None,
        ),
    ];

    for (name, metrics, expected) in cases {
        assert_eq!(metrics.output_tokens_per_second(), expected, "{name}");
    }
}

// Covers: generation throughput excludes TTFT and needs a streamed interval.
// Owner: SDK model-call metrics
#[test]
fn generation_rate_divides_tokens_by_generation_time() {
    let cases = [
        (
            "rate uses only the post-first-event window",
            ModelCallMetrics {
                output_tokens: Some(100),
                time_to_first_token: Some(Duration::from_secs(8)),
                generation_time: Some(Duration::from_secs(2)),
                total_latency: Duration::from_secs(10),
            },
            Some(50.0),
        ),
        (
            "no streamed generation means no generation rate",
            ModelCallMetrics {
                output_tokens: Some(50),
                time_to_first_token: None,
                generation_time: None,
                total_latency: Duration::from_secs(5),
            },
            None,
        ),
        (
            "no reported output tokens means no generation rate",
            ModelCallMetrics {
                output_tokens: None,
                time_to_first_token: Some(Duration::from_secs(1)),
                generation_time: Some(Duration::from_secs(2)),
                total_latency: Duration::from_secs(3),
            },
            None,
        ),
        (
            "a zero-length generation window means no rate",
            ModelCallMetrics {
                output_tokens: Some(50),
                time_to_first_token: Some(Duration::from_secs(1)),
                generation_time: Some(Duration::ZERO),
                total_latency: Duration::from_secs(1),
            },
            None,
        ),
    ];

    for (name, metrics, expected) in cases {
        assert_eq!(metrics.generation_tokens_per_second(), expected, "{name}");
    }
}
