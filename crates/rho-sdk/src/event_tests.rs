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

// Covers: manually constructed legacy variants cannot disagree with hook or model status.
// Owner: SDK completion projections; includes both variants and non-output completions.
#[test]
fn completion_status_uses_output_flag_for_model_and_hooks() {
    use crate::{
        hooks::{tool_status, HookToolStatus, ToolOutcomeRef},
        model::ToolResult,
        tool::{ToolError, ToolErrorKind, ToolOutput},
        ToolCompletion,
    };

    let successful = ToolOutput::text("result");
    let failed = successful.clone().failed();
    for (completion, output, failure, hook_status, content) in [
        (
            ToolCompletion::Success(successful.clone()),
            Some(successful.clone()),
            false,
            HookToolStatus::Succeeded,
            "result",
        ),
        (
            ToolCompletion::Success(failed.clone()),
            Some(failed.clone()),
            true,
            HookToolStatus::Failed,
            "result",
        ),
        (
            ToolCompletion::CompletedFailure(successful.clone()),
            Some(successful.clone()),
            false,
            HookToolStatus::Succeeded,
            "result",
        ),
        (
            ToolCompletion::CompletedFailure(failed.clone()),
            Some(failed.clone()),
            true,
            HookToolStatus::Failed,
            "result",
        ),
        (
            ToolCompletion::from_result(Err(ToolError::new(ToolErrorKind::Execution, "error"))),
            None,
            true,
            HookToolStatus::Failed,
            "error",
        ),
        (
            ToolCompletion::Unavailable,
            None,
            true,
            HookToolStatus::Unavailable,
            "tool 'test' is unavailable",
        ),
    ] {
        assert_eq!(completion.output(), output.as_ref());
        assert_eq!(completion.is_failure(), failure);
        assert_eq!(
            tool_status(ToolOutcomeRef::from(&completion)).0,
            hook_status
        );
        assert_eq!(
            completion.model_result("test", "call"),
            ToolResult {
                id: "call".into(),
                ok: !failure,
                content: content.into(),
            }
        );
        if let Some(output) = output {
            assert_eq!(ToolCompletion::from_output(output).is_failure(), failure);
        }
    }
}
