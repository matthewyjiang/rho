use std::sync::Mutex;

use pretty_assertions::assert_eq;
use rho_sdk::{ToolHost, ToolHostCall};

use crate::workflow::RunLifecycle;

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
            Ok(WorkflowToolResult::Run {
                run_id: RUN_ID.into(),
                graph_digest: "sha256:test".into(),
                state: RunLifecycle::Running,
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
        output.content(),
        format!("workflow {RUN_ID}: running\ngraph_digest: sha256:test\nnodes: 0")
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
