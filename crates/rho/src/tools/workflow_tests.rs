use std::sync::Mutex;

use pretty_assertions::assert_eq;
use rho_sdk::{ToolHost, ToolHostCall};

use crate::workflow::{ArtifactKind, ArtifactObservation};

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

// Covers: each workflow operation must expose its useful result fields as a
// readable line summary rather than model-facing JSON.
// Owner: model-facing workflow tool formatter.
#[test]
fn workflow_results_are_readable_line_summaries() {
    let cases = vec![
        (
            "validate",
            WorkflowToolResult::Validate {
                valid: false,
                diagnostics: vec![WorkflowDiagnosticSummary {
                    severity: "error".into(),
                    code: "invalid_graph".into(),
                    message: "cycle found\ncheck dependencies".into(),
                    source: Some("review.star".into()),
                    line: Some(7),
                    column: Some(3),
                }],
            },
            concat!(
                "workflow validation: invalid\n",
                "diagnostics:\n",
                "  error [invalid_graph]: cycle found\n",
                "    check dependencies\n",
                "    source: review.star\n",
                "    line: 7\n",
                "    column: 3"
            ),
        ),
        (
            "plan",
            WorkflowToolResult::Plan {
                plan_id: "plan-1".into(),
                graph_digest: "sha256:plan".into(),
                workflow_name: "review".into(),
                node_count: 2,
            },
            concat!(
                "workflow review: planned\n",
                "plan_id: plan-1\n",
                "graph_digest: sha256:plan\n",
                "nodes: 2"
            ),
        ),
        (
            "run",
            WorkflowToolResult::Run {
                run_id: "run-1".into(),
                graph_digest: "sha256:run".into(),
                state: WorkflowRunStateSummary::Running,
                nodes: vec![WorkflowNodeSummary {
                    node_id: "build".into(),
                    state: WorkflowNodeStateSummary::Running,
                    attempt: Some(2),
                    artifacts: vec![WorkflowArtifactSummary {
                        kind: ArtifactKind::Stdout,
                        artifact: crate::workflow::ArtifactRef {
                            relative_path: "artifacts/build/stdout".into(),
                            retained_bytes: 8,
                            observed: ArtifactObservation::Truncated {
                                observed_bytes_at_least: 12,
                            },
                            digest: crate::workflow::Digest("sha256:artifact".into()),
                        },
                    }],
                }],
            },
            concat!(
                "workflow run-1: running\n",
                "graph_digest: sha256:run\n",
                "nodes: 1\n",
                "  build · running · attempt 2\n",
                "    stdout: artifacts/build/stdout · 8 bytes retained · ",
                "at least 12 bytes observed (truncated) · digest sha256:artifact"
            ),
        ),
        (
            "status",
            WorkflowToolResult::Status {
                run_id: "run-2".into(),
                graph_digest: "sha256:status".into(),
                state: WorkflowRunStateSummary::NeedsRecovery,
                nodes: Vec::new(),
            },
            concat!(
                "workflow run-2: needs_recovery\n",
                "graph_digest: sha256:status\n",
                "nodes: 0"
            ),
        ),
        (
            "cancel",
            WorkflowToolResult::Cancel {
                run_id: "run-3".into(),
                request_id: Some("request-1".into()),
                cancellation_state: WorkflowCancellationStateSummary::Pending,
                state: WorkflowRunStateSummary::Running,
            },
            concat!(
                "workflow run-3: running\n",
                "cancellation: pending\n",
                "request_id: request-1"
            ),
        ),
        (
            "resume",
            WorkflowToolResult::Resume {
                run_id: "run-4".into(),
                graph_digest: "sha256:resume".into(),
                state: WorkflowRunStateSummary::Running,
                nodes: vec![WorkflowNodeSummary {
                    node_id: "verify".into(),
                    state: WorkflowNodeStateSummary::Pending,
                    attempt: None,
                    artifacts: Vec::new(),
                }],
            },
            concat!(
                "workflow run-4: running\n",
                "graph_digest: sha256:resume\n",
                "nodes: 1\n",
                "  verify · pending"
            ),
        ),
    ];

    for (operation, result, expected) in cases {
        assert_eq!(
            format_workflow_result(&result),
            expected,
            "{operation} result"
        );
    }
}

// Covers: oversized workflow summaries must stay within the configured byte
// limit while retaining a user-facing prefix and truncation notice when possible.
// Owner: model-facing workflow tool formatter.
#[test]
fn bounded_results_retain_the_readable_prefix() {
    let valid = WorkflowToolResult::Validate {
        valid: true,
        diagnostics: Vec::new(),
    };
    assert_eq!(
        bounded_result(&valid, 256).unwrap(),
        "workflow validation: valid"
    );

    let oversized = WorkflowToolResult::Validate {
        valid: false,
        diagnostics: vec![WorkflowDiagnosticSummary {
            severity: "error".into(),
            code: "invalid".into(),
            message: "é".repeat(4096),
            source: None,
            line: None,
            column: None,
        }],
    };
    let formatted = format_workflow_result(&oversized);
    let max_output_bytes = 256;
    let notice = format!(
        "... workflow details omitted: output is {} bytes; limit is {max_output_bytes} bytes",
        formatted.len()
    );
    let output = bounded_result(&oversized, max_output_bytes).unwrap();

    assert!(output.len() <= max_output_bytes);
    assert_eq!(
        output,
        format!("workflow validation: invalid\ndiagnostics:\n{notice}")
    );

    let tiny_limit = 4;
    let tiny_notice = format!(
        "... workflow details omitted: output is {} bytes; limit is {tiny_limit} bytes",
        formatted.len()
    );
    let error = bounded_result(&oversized, tiny_limit).unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert_eq!(
        error.message(),
        format!(
            "workflow tool output budget is too small: accepted limit {tiny_limit}, required {}",
            tiny_notice.len()
        )
    );
}
