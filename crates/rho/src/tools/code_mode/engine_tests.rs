use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{bridge::ToolHostBridge, exposure::CodeModeSurface};
use super::{
    evaluate_code_mode, format_engine_output, CapturedPrints, EngineLimits, EngineOutput,
    Evaluation,
};

async fn evaluate(source: String, cancellation: rho_sdk::CancellationToken) -> Evaluation {
    let (progress, receiver) = rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
    drop(receiver);
    let bridge = std::sync::Arc::new(
        ToolHostBridge::new(
            std::sync::Arc::new(CodeModeSurface::default()),
            rho_sdk::tool::ToolContext::new(/*workspace*/ None, cancellation, progress),
        )
        .unwrap(),
    );
    tokio::task::spawn_blocking(move || {
        evaluate_code_mode(&source, bridge, EngineLimits::default())
    })
    .await
    .unwrap()
}

// Covers: prints and pretty-printed return values share one hard byte cap,
// including UTF-8 boundaries, the notice, and the truncation marker. Failures
// keep their complete diagnostic even when prints consume the entire budget.
// Owner: codemode model-facing output formatting.
#[tokio::test]
async fn engine_output_has_one_total_byte_budget() {
    let limit = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
    let failed = evaluate(
        format!("print(\"é\" * {limit})\nfail(\"boom\")"),
        rho_sdk::CancellationToken::new(),
    )
    .await;
    let diagnostic = format!("{:#}", failed.result.unwrap_err());
    for output in [
        EngineOutput {
            return_value: json!("é".repeat(limit)),
            prints: Vec::new(),
            calls: 0,
            error: None,
        },
        EngineOutput {
            return_value: json!("returned"),
            prints: vec!["é".repeat(limit / 2)],
            calls: 0,
            error: None,
        },
        EngineOutput {
            return_value: serde_json::Value::Null,
            prints: vec!["x".repeat(limit)],
            calls: 0,
            error: None,
        },
        EngineOutput {
            return_value: serde_json::Value::Null,
            prints: failed.prints,
            calls: 0,
            error: Some(diagnostic),
        },
        EngineOutput {
            return_value: serde_json::Value::Null,
            prints: Vec::new(),
            calls: 0,
            error: Some(format!("diagnostic head\n{}", "é".repeat(limit))),
        },
    ] {
        let rendered = format_engine_output(&output);
        assert!(rendered.len() <= limit, "{} > {limit}", rendered.len());
        if let Some(error) = &output.error {
            assert!(error.lines().count() > 1, "missing Starlark diagnostic");
            let suffix = rendered.rsplit_once("script failed: ").unwrap().1;
            if error.len() <= limit {
                assert_eq!(suffix, error);
                assert!(rendered.ends_with(&format!("\n\nscript failed: {error}")));
                assert!(rendered.starts_with('é'));
            } else {
                let head = suffix
                    .strip_suffix(rho_tools::tool::TRUNCATION_MARKER)
                    .unwrap();
                assert!(error.starts_with(head));
                assert!(head.starts_with("diagnostic head\n"));
            }
        } else if rendered.len() == limit && output.return_value.is_null() {
            assert_eq!(rendered, output.prints[0]);
        } else {
            assert!(rendered.ends_with(rho_tools::tool::TRUNCATION_MARKER));
        }
    }
}

// Covers: print capture bounds allocations before evaluation finishes, including
// empty lines, UTF-8 splits, and discarded tail bytes.
// Owner: codemode evaluator print handler.
#[test]
fn print_capture_bounds_storage_and_preserves_prefix() {
    let limit = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
    for input in [
        vec!["a".to_owned(), "b\nc".to_owned()],
        vec!["x".repeat(limit)],
        vec!["x".repeat(limit + 1)],
        vec!["é".repeat(limit / 2 + 1)],
        vec![String::new(); limit + 2],
        vec!["x".repeat(limit), "tail".repeat(limit)],
    ] {
        let received = input.join("\n");
        let mut capture = CapturedPrints::default();
        for line in &input {
            capture.push(line);
        }
        assert!(capture.retained_bytes <= limit);
        assert!(capture.lines.len() <= limit + 1);
        assert_eq!(capture.received_bytes, received.len());
        let lines = capture.into_lines();
        assert!(lines.join("\n").len() <= limit);
        if received.len() <= limit {
            assert_eq!(lines, input);
        } else {
            let notice = format!(
                "[codemode prints truncated: output byte limit {limit}, received {} bytes]",
                received.len()
            );
            let prefix = super::utf8_prefix(&received, limit - notice.len() - 1);
            assert_eq!(lines.join("\n"), format!("{prefix}\n{notice}"));
        }
    }
}

// Covers: cancelling the parent interrupts pure computation, not just native
// waits. A tick-budget error would mean the evaluator ignored cancellation.
// Owner: codemode evaluator cancellation hook.
#[tokio::test]
async fn parent_cancellation_interrupts_script_without_nested_calls() {
    let cancellation = rho_sdk::CancellationToken::new();
    cancellation.cancel();
    let evaluation = evaluate(
        "total = 0\nfor i in range(10000000):\n    total += i\nresult = total".to_owned(),
        cancellation,
    )
    .await;
    assert_eq!(
        evaluation
            .result
            .unwrap_err()
            .without_diagnostic()
            .to_string(),
        "Evaluation cancelled"
    );
}
