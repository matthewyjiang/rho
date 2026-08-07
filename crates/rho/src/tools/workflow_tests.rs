use std::sync::Mutex;

use pretty_assertions::assert_eq;
use rho_sdk::{ToolHost, ToolHostCall};

use super::*;

const RUN_ID: &str = "00000000-0000-4000-8000-000000000001";

fn run_id() -> RunId {
    RUN_ID.parse().expect("canonical run id")
}

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
                run_id: RUN_ID.into(),
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
            serde_json::json!({"action": "status", "run_id": RUN_ID}),
        ))
        .unwrap();
    let output = run.outcome().await.unwrap();

    assert_eq!(
        service.requests.lock().unwrap().as_slice(),
        &[WorkflowToolRequest::Status { run_id: run_id() }]
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(output.content()).unwrap(),
        serde_json::json!({
            "operation": "status",
            "run_id": RUN_ID,
            "graph_digest": "sha256:test",
            "state": "running",
            "nodes": []
        })
    );
}

// Covers: workflow tool advertises a portable flat multi-action object schema.
// Owner: model-facing workflow tool adapter.
#[test]
fn workflow_tool_schema_is_a_root_object() {
    let schema = WorkflowTool::new(Arc::new(RecordingService::default()), 4096)
        .spec()
        .input_schema;
    assert_eq!(schema["type"], "object");
    assert!(schema.get("oneOf").is_none());
    assert_eq!(schema["required"], serde_json::json!(["action"]));
    assert_eq!(
        schema["properties"]["action"]["enum"],
        serde_json::json!(["validate", "plan", "run", "status", "cancel", "resume"])
    );
}

// Covers: model-facing workflow JSON must be readable without bypassing the configured output bound.
// Owner: model-facing workflow tool serializer.
#[test]
fn bounded_results_are_pretty_and_respect_the_output_budget() {
    let result = WorkflowToolResult::Validate {
        valid: true,
        diagnostics: Vec::new(),
    };
    assert_eq!(
        bounded_result(&result, 256).unwrap(),
        concat!(
            "{\n",
            "  \"operation\": \"validate\",\n",
            "  \"valid\": true,\n",
            "  \"diagnostics\": []\n",
            "}"
        )
    );

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
    let requested_bytes = serde_json::to_string_pretty(&result).unwrap().len();

    let output = bounded_result(&result, 256).unwrap();

    assert!(output.len() <= 256);
    assert_eq!(
        output,
        serde_json::to_string_pretty(&serde_json::json!({
            "truncated": true,
            "reason": "workflow summary exceeded the tool output budget",
            "requested_bytes": requested_bytes,
            "limit_bytes": 256,
        }))
        .unwrap()
    );
}

// Covers: model cancellation must preserve the exact durable request ID and
// typed acknowledgement state returned by the application service.
// Owner: model-facing workflow tool result wire format.
#[test]
fn cancel_result_exposes_exact_request_state() {
    let result = WorkflowToolResult::Cancel {
        run_id: "00000000-0000-0000-0000-000000000001".into(),
        request_id: Some("00000000-0000-0000-0000-000000000002".into()),
        cancellation_state: WorkflowCancellationStateSummary::Pending,
        state: WorkflowRunStateSummary::Running,
    };

    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::json!({
            "operation": "cancel",
            "run_id": "00000000-0000-0000-0000-000000000001",
            "request_id": "00000000-0000-0000-0000-000000000002",
            "cancellation_state": "pending",
            "state": "running"
        })
    );
}
