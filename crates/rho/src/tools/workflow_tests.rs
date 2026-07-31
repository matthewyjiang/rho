use std::sync::Mutex;

use pretty_assertions::assert_eq;
use rho_sdk::{ToolHost, ToolHostCall};

use super::*;

#[derive(Default)]
struct RecordingService {
    requests: Mutex<Vec<WorkflowToolRequest>>,
}

impl WorkflowToolService for RecordingService {
    fn execute<'a>(
        &'a self,
        request: WorkflowToolRequest,
        _context: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowToolResult, ToolError>> + Send + 'a>> {
        self.requests.lock().unwrap().push(request.clone());
        Box::pin(async move {
            Ok(WorkflowToolResult::Status {
                run_id: "run-1".into(),
                graph_digest: "sha256:test".into(),
                state: WorkflowRunStateSummary::Running,
                nodes: Vec::new(),
            })
        })
    }
}

// Covers: model JSON must map to one typed workflow operation before service dispatch.
// Owner: model-facing workflow tool adapter.
#[tokio::test]
async fn dispatches_a_typed_operation() {
    let service = Arc::new(RecordingService::default());
    let tool = WorkflowTool::new(service.clone(), 4096);
    let host = ToolHost::builder().tool(tool).build().unwrap();

    let mut run = host
        .start(ToolHostCall::new(
            NAME,
            serde_json::json!({"action": "status", "run_id": "run-1"}),
        ))
        .unwrap();
    let output = run.outcome().await.unwrap();

    assert_eq!(
        service.requests.lock().unwrap().as_slice(),
        &[WorkflowToolRequest::Status {
            run_id: "run-1".into()
        }]
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(output.content()).unwrap(),
        serde_json::json!({
            "operation": "status",
            "run_id": "run-1",
            "graph_digest": "sha256:test",
            "state": "running",
            "nodes": []
        })
    );
}

// Covers: large workflow state must not bypass the configured tool output bound.
// Owner: model-facing workflow tool adapter.
#[test]
fn oversized_results_are_replaced_with_a_bounded_summary() {
    let result = WorkflowToolResult::Validate {
        valid: false,
        diagnostics: vec![WorkflowDiagnosticSummary {
            severity: "error".into(),
            code: "invalid".into(),
            message: "x".repeat(4096),
            source: None,
            line: None,
            column: None,
        }],
    };

    let output = bounded_result(&result, 256).unwrap();

    assert!(output.len() <= 256);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap()["truncated"],
        true
    );
}
