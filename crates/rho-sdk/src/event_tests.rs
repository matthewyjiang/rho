use std::time::Duration;

use super::ModelCallMetrics;

// Covers: hidden reasoning tokens must include pre-stream reasoning time in their rate.
// Owner: SDK model-call metrics
#[test]
fn output_rate_uses_total_model_call_latency() {
    let metrics = ModelCallMetrics {
        output_tokens: Some(100),
        time_to_first_token: Some(Duration::from_secs(8)),
        generation_time: Some(Duration::from_secs(2)),
        total_latency: Duration::from_secs(10),
    };

    assert_eq!(metrics.end_to_end_output_tokens_per_second(), Some(10.0));
    assert_eq!(metrics.output_tokens_per_second(), Some(10.0));
}
