use std::time::Duration;

use pretty_assertions::assert_eq;

use super::ModelCallMetrics;

// Covers: end-to-end response rate keeps pre-stream time in the denominator.
// Owner: SDK model-call metrics
#[test]
fn response_rate_divides_tokens_by_total_latency() {
    let cases = [
        (
            "reasoning before the first event is charged to the rate",
            ModelCallMetrics {
                output_tokens: Some(100),
                time_to_first_token: Some(Duration::from_secs(8)),
                generation_time: Some(Duration::from_secs(2)),
                total_latency: Duration::from_secs(10),
                generation_output_tokens: None,
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
                generation_output_tokens: None,
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
                generation_output_tokens: None,
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
                generation_output_tokens: None,
            },
            None,
        ),
    ];

    for (name, metrics, expected) in cases {
        assert_eq!(metrics.response_tokens_per_second(), expected, "{name}");
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
                generation_output_tokens: None,
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
                generation_output_tokens: None,
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
                generation_output_tokens: None,
            },
            None,
        ),
        (
            "unavailable generation tokens suppress the aggregate fallback",
            ModelCallMetrics {
                output_tokens: Some(100),
                time_to_first_token: Some(Duration::from_secs(1)),
                generation_time: Some(Duration::from_secs(2)),
                total_latency: Duration::from_secs(3),
                generation_output_tokens: Some(crate::model::GenerationOutputTokens::Unavailable),
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
                generation_output_tokens: None,
            },
            None,
        ),
    ];

    for (name, metrics, expected) in cases {
        assert_eq!(metrics.generation_tokens_per_second(), expected, "{name}");
    }
}

// Covers: inconsistent legacy variants cannot disagree with hook or model status.
// Owner: SDK completion projections, including non-output completions.
#[test]
fn completion_status_uses_output_flag_for_model_and_hooks() {
    use crate::{
        hooks::{
            tool_status,
            HookToolStatus::{Failed, Succeeded, Unavailable as UnavailableStatus},
            ToolOutcomeRef,
        },
        tool::{ToolError, ToolErrorKind, ToolOutput},
        ToolCompletion::{self, CompletedFailure, Success},
    };

    let successful = ToolOutput::text("result");
    let failed = successful.clone().failed();
    let error = ToolCompletion::from_result(Err(ToolError::new(ToolErrorKind::Execution, "error")));
    for (completion, expected) in [
        (Success(successful.clone()), (false, true, Succeeded)),
        (Success(failed.clone()), (true, false, Failed)),
        (
            CompletedFailure(successful.clone()),
            (false, true, Succeeded),
        ),
        (CompletedFailure(failed.clone()), (true, false, Failed)),
        (
            ToolCompletion::from_output(successful),
            (false, true, Succeeded),
        ),
        (ToolCompletion::from_output(failed), (true, false, Failed)),
        (error, (true, false, Failed)),
        (
            ToolCompletion::Unavailable,
            (true, false, UnavailableStatus),
        ),
    ] {
        let result = completion.model_result("test", "call");
        assert_eq!(
            (
                completion.is_failure(),
                result.ok,
                tool_status(ToolOutcomeRef::from(&completion)).0,
            ),
            expected
        );
        let (output, content) = match &completion {
            Success(output) | CompletedFailure(output) => (Some(output), "result"),
            ToolCompletion::Failure(_) => (None, "error"),
            ToolCompletion::Unavailable => (None, "tool 'test' is unavailable"),
        };
        assert_eq!(completion.output(), output);
        assert_eq!(
            (result.id.as_str(), result.content.as_str()),
            ("call", content)
        );
    }
}
