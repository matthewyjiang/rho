use std::time::Duration;

use pretty_assertions::assert_eq;

use super::ModelCallMetrics;

// Covers: hidden reasoning tokens must include pre-stream reasoning time in their rate,
// and retry backoff must not deflate it.
// Owner: SDK model-call metrics
#[test]
fn output_rate_uses_attempt_latency() {
    let cases = [
        (
            "reasoning before the first event is charged to the rate",
            ModelCallMetrics {
                output_tokens: Some(100),
                time_to_first_token: Some(Duration::from_secs(8)),
                generation_time: Some(Duration::from_secs(2)),
                attempt_latency: Duration::from_secs(10),
                total_latency: Duration::from_secs(10),
            },
            Some(10.0),
        ),
        (
            "retry backoff before the winning attempt is not charged",
            ModelCallMetrics {
                output_tokens: Some(100),
                time_to_first_token: Some(Duration::from_secs(8)),
                generation_time: Some(Duration::from_secs(2)),
                attempt_latency: Duration::from_secs(10),
                total_latency: Duration::from_secs(40),
            },
            Some(10.0),
        ),
        (
            "a call that never streamed still reports a rate",
            ModelCallMetrics {
                output_tokens: Some(50),
                time_to_first_token: None,
                generation_time: None,
                attempt_latency: Duration::from_secs(5),
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
                attempt_latency: Duration::from_secs(5),
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
                attempt_latency: Duration::ZERO,
                total_latency: Duration::ZERO,
            },
            None,
        ),
    ];

    for (name, metrics, expected) in cases {
        assert_eq!(metrics.output_tokens_per_second(), expected, "{name}");
    }
}
