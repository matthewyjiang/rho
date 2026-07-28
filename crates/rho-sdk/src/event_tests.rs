use std::time::Duration;

use pretty_assertions::assert_eq;

use super::ModelCallMetrics;

// Covers: hidden reasoning tokens must include pre-stream reasoning time in their rate.
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
