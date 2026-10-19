use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{ToolHost, ToolHostCall};
use serde_json::json;

use super::{CodeModeSurface, CodeModeTool, CODEMODE_TOOL_NAME};

// Covers: bounded model text must not leave an unbounded duplicate structured
// result in session storage; omission must preserve completed failure status.
// Owner: codemode ToolHost output boundary (format-only tests cannot cover data).
#[tokio::test]
async fn engine_output_bounds_structured_data_before_persistence() {
    let host = ToolHost::builder()
        .tool(CodeModeTool::new(Arc::new(CodeModeSurface::default())))
        .build()
        .unwrap();
    let limit = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
    for (source, expected_data, failed) in [
        (
            "print(\"before\")\nresult = {\"value\": 1}".to_owned(),
            Some(json!({"return_value": {"value": 1}, "prints": ["before"], "calls": 0})),
            false,
        ),
        (format!("result = \"é\" * {limit}"), None, false),
        (format!("print(\"é\" * {limit})"), None, false),
        (
            format!("print(\"é\" * {limit})\nfail(\"boom\")"),
            None,
            true,
        ),
    ] {
        let diagnostic = if failed {
            use super::super::{
                bridge::ToolHostBridge,
                engine::{evaluate_code_mode, EngineLimits},
            };
            let (progress, receiver) =
                rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
            drop(receiver);
            let bridge = Arc::new(
                ToolHostBridge::new(
                    Arc::new(CodeModeSurface::default()),
                    rho_sdk::tool::ToolContext::new(
                        /*workspace*/ None,
                        rho_sdk::CancellationToken::new(),
                        progress,
                    ),
                )
                .unwrap(),
            );
            let source = source.clone();
            let evaluation = tokio::task::spawn_blocking(move || {
                evaluate_code_mode(&source, bridge, EngineLimits::default())
            })
            .await
            .unwrap();
            Some(format!("{:#}", evaluation.result.unwrap_err()))
        } else {
            None
        };
        let output = host
            .invoke(ToolHostCall::new(
                CODEMODE_TOOL_NAME,
                json!({"script": source}),
            ))
            .await
            .unwrap();
        assert_eq!(
            (output.structured_content(), output.is_failure()),
            (expected_data.as_ref(), failed)
        );
        assert!(output.content().len() <= limit);
        if let Some(diagnostic) = diagnostic {
            assert!(
                output
                    .content()
                    .ends_with(&format!("script failed: {diagnostic}")),
                "complete failure diagnostic must survive structured-data omission"
            );
        }
        if let Some(data) = output.structured_content() {
            assert!(serde_json::to_vec(data).unwrap().len() <= limit);
        } else {
            // This wire notice names the budget, its limit, and the asked size.
            let asked = output
                .content()
                .lines()
                .next()
                .unwrap()
                .strip_prefix(&format!(
                    "[structured data truncated: max_output_bytes {limit}, received "
                ))
                .unwrap()
                .strip_suffix(" bytes]")
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert!(asked > limit);
        }
    }
}
