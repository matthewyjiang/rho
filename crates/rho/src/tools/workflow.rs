//! Model-facing workflow orchestration tool.

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use rho_sdk::tool::{
    OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind,
    ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    ToolSecurity,
};
use serde::{Deserialize, Serialize};

use crate::workflow::{PlanId, RunId};

use super::sdk_registry::StaticToolBundle;

pub(crate) const NAME: &str = "workflow";

pub(crate) fn sdk_bundle(
    service: Arc<dyn WorkflowToolService>,
    max_output_bytes: usize,
) -> StaticToolBundle {
    StaticToolBundle::new(vec![Arc::new(WorkflowTool::new(service, max_output_bytes))])
}

/// Typed operations accepted by the model-facing workflow tool.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkflowToolRequest {
    Validate {
        file: String,
        #[serde(default)]
        inputs: BTreeMap<String, serde_json::Value>,
    },
    Plan {
        file: String,
        #[serde(default)]
        inputs: BTreeMap<String, serde_json::Value>,
    },
    Run {
        plan_id: PlanId,
    },
    Status {
        run_id: RunId,
    },
    Cancel {
        run_id: RunId,
    },
    Resume {
        run_id: RunId,
        #[serde(default)]
        recover_uncertain: bool,
    },
}

impl WorkflowToolRequest {
    fn operation(&self) -> OperationKind {
        match self {
            Self::Validate { .. } | Self::Status { .. } => OperationKind::Read,
            Self::Plan { .. } | Self::Run { .. } | Self::Cancel { .. } | Self::Resume { .. } => {
                OperationKind::Other("workflow".into())
            }
        }
    }
}

/// A bounded artifact pointer. Workflow tool results never contain artifact data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WorkflowArtifactSummary {
    pub(crate) kind: crate::workflow::ArtifactKind,
    #[serde(flatten)]
    pub(crate) artifact: crate::workflow::ArtifactRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowRunStateSummary {
    Planned,
    Running,
    Cancelling,
    Completed,
    NeedsRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowCancellationStateSummary {
    Acknowledged,
    Pending,
    AlreadyCompleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowNodeStateSummary {
    Pending,
    Ready,
    Running,
    Success,
    Failure,
    Denial,
    Cancellation,
    Skipped,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WorkflowNodeSummary {
    pub(crate) node_id: String,
    pub(crate) state: WorkflowNodeStateSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifacts: Vec<WorkflowArtifactSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WorkflowDiagnosticSummary {
    pub(crate) severity: String,
    pub(crate) code: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) column: Option<u64>,
}

/// Typed summaries returned by the shared workflow application service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub(crate) enum WorkflowToolResult {
    Validate {
        valid: bool,
        diagnostics: Vec<WorkflowDiagnosticSummary>,
    },
    Plan {
        plan_id: String,
        graph_digest: String,
        workflow_name: String,
        node_count: u64,
    },
    Run {
        run_id: String,
        graph_digest: String,
        state: WorkflowRunStateSummary,
        nodes: Vec<WorkflowNodeSummary>,
    },
    Status {
        run_id: String,
        graph_digest: String,
        state: WorkflowRunStateSummary,
        nodes: Vec<WorkflowNodeSummary>,
    },
    Cancel {
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        cancellation_state: WorkflowCancellationStateSummary,
        state: WorkflowRunStateSummary,
    },
    Resume {
        run_id: String,
        graph_digest: String,
        state: WorkflowRunStateSummary,
        nodes: Vec<WorkflowNodeSummary>,
    },
}

/// App service used by both the CLI adapter and this model-facing adapter.
///
/// The service owns source collection, planning, persistence, confirmation,
/// running, cancellation, and resume. It must authorize each collected source
/// read through `context`. Run and resume must request host input for the exact
/// graph digest and fail closed when no responder exists.
pub(crate) trait WorkflowToolService: Send + Sync {
    fn prepare(
        &self,
        _request: &WorkflowToolRequest,
    ) -> Result<Vec<rho_sdk::CapabilityRequest>, ToolError> {
        Ok(Vec::new())
    }

    fn execute<'a>(
        &'a self,
        request: WorkflowToolRequest,
        context: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowToolResult, ToolError>> + Send + 'a>>;
}

pub(crate) struct WorkflowTool {
    service: Arc<dyn WorkflowToolService>,
    max_output_bytes: usize,
}

impl WorkflowTool {
    pub(crate) fn new(service: Arc<dyn WorkflowToolService>, max_output_bytes: usize) -> Self {
        Self {
            service,
            max_output_bytes: max_output_bytes.max(1),
        }
    }
}

impl Tool for WorkflowTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: NAME.into(),
            description: "Validate, freeze, run, inspect, cancel, or resume a Rho workflow. Run and resume start in the background and return immediately with a run id. Completions arrive automatically at the next turn boundary (batched); do not poll in a loop. Use status for a live check or after delivery, and cancel to stop. Results are bounded summaries; read artifacts separately.".into(),
            input_schema: workflow_schema(),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([
            rho_sdk::CapabilityKind::Read,
            rho_sdk::CapabilityKind::Write,
            rho_sdk::CapabilityKind::Process,
        ])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            let request: WorkflowToolRequest =
                serde_json::from_value(invocation.arguments().clone()).map_err(|error| {
                    ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        format!("invalid workflow operation: {error}"),
                    )
                })?;
            let operation = request.operation();
            let capabilities = self.service.prepare(&request)?;
            let service = Arc::clone(&self.service);
            let max_output_bytes = self.max_output_bytes;
            Ok(PreparedToolInvocation::exclusive_with_capabilities(
                capabilities,
                ToolMetadata::new().operation(operation.clone()),
                move |context| {
                    Box::pin(async move {
                        let result = service.execute(request, &context).await?;
                        let content = bounded_result(&result, max_output_bytes)?;
                        Ok(ToolOutput::text(content)
                            .metadata(ToolMetadata::new().operation(operation)))
                    })
                },
            ))
        })
    }
}

fn bounded_result(
    result: &WorkflowToolResult,
    max_output_bytes: usize,
) -> Result<String, ToolError> {
    let serialized = serde_json::to_string_pretty(result)
        .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
    if serialized.len() <= max_output_bytes {
        return Ok(serialized);
    }
    let summary = serde_json::to_string_pretty(&serde_json::json!({
        "truncated": true,
        "reason": "workflow summary exceeded the tool output budget",
        "requested_bytes": serialized.len(),
        "limit_bytes": max_output_bytes,
    }))
    .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
    if summary.len() <= max_output_bytes {
        return Ok(summary);
    }
    Err(ToolError::new(
        ToolErrorKind::Execution,
        format!(
            "workflow tool output budget is too small: accepted limit {max_output_bytes}, required {}",
            summary.len()
        ),
    ))
}

fn workflow_schema() -> serde_json::Value {
    // Flat multi-action object: action enum + optional fields.
    // Per-action required fields are enforced by WorkflowToolRequest.
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["validate", "plan", "run", "status", "cancel", "resume"],
                "description": "validate/plan need file; run needs plan_id; status/cancel/resume need run_id."
            },
            "file": {
                "type": "string",
                "minLength": 1,
                "description": "Workflow source path for validate and plan."
            },
            "inputs": {
                "type": "object",
                "description": "Explicit input values. Inputs are persisted and are not a secret store."
            },
            "plan_id": {
                "type": "string",
                "minLength": 1,
                "description": "Frozen plan id for run."
            },
            "run_id": {
                "type": "string",
                "minLength": 1,
                "description": "Run id for status, cancel, and resume."
            },
            "recover_uncertain": {
                "type": "boolean",
                "description": "Confirm no prior process remains before relaunching uncertain attempts."
            }
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
