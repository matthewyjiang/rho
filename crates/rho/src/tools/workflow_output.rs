use rho_sdk::tool::{ToolError, ToolErrorKind};

use crate::workflow::{ArtifactKind, ArtifactObservation};

use super::{
    WorkflowCancellationStateSummary, WorkflowNodeStateSummary, WorkflowNodeSummary,
    WorkflowRunStateSummary, WorkflowToolResult,
};

pub(super) fn format_workflow_result(result: &WorkflowToolResult) -> String {
    let mut lines = Vec::new();
    match result {
        WorkflowToolResult::Validate { valid, diagnostics } => {
            lines.push(format!(
                "workflow validation: {}",
                if *valid { "valid" } else { "invalid" }
            ));
            if !diagnostics.is_empty() {
                lines.push("diagnostics:".into());
            }
            for diagnostic in diagnostics {
                let label = format!("{} [{}]", diagnostic.severity, diagnostic.code);
                push_labeled_lines(&mut lines, "  ", &label, &diagnostic.message);
                if let Some(source) = &diagnostic.source {
                    push_labeled_lines(&mut lines, "    ", "source", source);
                }
                if let Some(line) = diagnostic.line {
                    lines.push(format!("    line: {line}"));
                }
                if let Some(column) = diagnostic.column {
                    lines.push(format!("    column: {column}"));
                }
            }
        }
        WorkflowToolResult::Plan {
            plan_id,
            graph_digest,
            workflow_name,
            node_count,
        } => {
            lines.push(format!("workflow {workflow_name}: planned"));
            lines.push(format!("plan_id: {plan_id}"));
            lines.push(format!("graph_digest: {graph_digest}"));
            lines.push(format!("nodes: {node_count}"));
        }
        WorkflowToolResult::Run {
            run_id,
            graph_digest,
            state,
            nodes,
        }
        | WorkflowToolResult::Status {
            run_id,
            graph_digest,
            state,
            nodes,
        }
        | WorkflowToolResult::Resume {
            run_id,
            graph_digest,
            state,
            nodes,
        } => push_run_summary(&mut lines, run_id, graph_digest, *state, nodes),
        WorkflowToolResult::Cancel {
            run_id,
            request_id,
            cancellation_state,
            state,
        } => {
            lines.push(format!("workflow {run_id}: {}", run_state_label(*state)));
            lines.push(format!(
                "cancellation: {}",
                cancellation_state_label(*cancellation_state)
            ));
            if let Some(request_id) = request_id {
                lines.push(format!("request_id: {request_id}"));
            }
        }
    }
    lines.join("\n")
}

fn push_run_summary(
    lines: &mut Vec<String>,
    run_id: &str,
    graph_digest: &str,
    state: WorkflowRunStateSummary,
    nodes: &[WorkflowNodeSummary],
) {
    lines.push(format!("workflow {run_id}: {}", run_state_label(state)));
    lines.push(format!("graph_digest: {graph_digest}"));
    lines.push(format!("nodes: {}", nodes.len()));
    for node in nodes {
        let attempt = node
            .attempt
            .map(|attempt| format!(" · attempt {attempt}"))
            .unwrap_or_default();
        lines.push(format!(
            "  {} · {}{attempt}",
            node.node_id,
            node_state_label(node.state)
        ));
        for artifact in &node.artifacts {
            let reference = &artifact.artifact;
            let observation = match &reference.observed {
                ArtifactObservation::Complete { observed_bytes } => {
                    format!("{observed_bytes} bytes observed (complete)")
                }
                ArtifactObservation::Truncated {
                    observed_bytes_at_least,
                } => format!("at least {observed_bytes_at_least} bytes observed (truncated)"),
                ArtifactObservation::Incomplete { observed_bytes } => {
                    format!("{observed_bytes} bytes observed (incomplete)")
                }
            };
            lines.push(format!(
                "    {}: {} · {} bytes retained · {observation} · digest {}",
                artifact_kind_label(artifact.kind),
                reference.relative_path,
                reference.retained_bytes,
                reference.digest.0
            ));
        }
    }
}

fn push_labeled_lines(lines: &mut Vec<String>, indent: &str, label: &str, value: &str) {
    let mut value_lines = value.lines();
    let Some(first) = value_lines.next() else {
        lines.push(format!("{indent}{label}:"));
        return;
    };
    lines.push(format!("{indent}{label}: {first}"));
    lines.extend(value_lines.map(|line| format!("{indent}  {line}")));
}

fn run_state_label(state: WorkflowRunStateSummary) -> &'static str {
    match state {
        WorkflowRunStateSummary::Planned => "planned",
        WorkflowRunStateSummary::Running => "running",
        WorkflowRunStateSummary::Cancelling => "cancelling",
        WorkflowRunStateSummary::Completed => "completed",
        WorkflowRunStateSummary::NeedsRecovery => "needs_recovery",
    }
}

fn cancellation_state_label(state: WorkflowCancellationStateSummary) -> &'static str {
    match state {
        WorkflowCancellationStateSummary::Acknowledged => "acknowledged",
        WorkflowCancellationStateSummary::Pending => "pending",
        WorkflowCancellationStateSummary::AlreadyCompleted => "already_completed",
    }
}

fn node_state_label(state: WorkflowNodeStateSummary) -> &'static str {
    match state {
        WorkflowNodeStateSummary::Pending => "pending",
        WorkflowNodeStateSummary::Ready => "ready",
        WorkflowNodeStateSummary::Running => "running",
        WorkflowNodeStateSummary::Success => "success",
        WorkflowNodeStateSummary::Failure => "failure",
        WorkflowNodeStateSummary::Denial => "denial",
        WorkflowNodeStateSummary::Cancellation => "cancellation",
        WorkflowNodeStateSummary::Skipped => "skipped",
        WorkflowNodeStateSummary::Blocked => "blocked",
    }
}

fn artifact_kind_label(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Stdout => "stdout",
        ArtifactKind::Stderr => "stderr",
        ArtifactKind::AgentAnswer => "agent answer",
        ArtifactKind::StructuredOutput => "structured output",
        ArtifactKind::CommandOutcome => "command outcome",
    }
}

pub(super) fn bounded_result(
    result: &WorkflowToolResult,
    max_output_bytes: usize,
) -> Result<String, ToolError> {
    let formatted = format_workflow_result(result);
    if formatted.len() <= max_output_bytes {
        return Ok(formatted);
    }

    let notice = format!(
        "... workflow details omitted: output is {} bytes; limit is {max_output_bytes} bytes",
        formatted.len()
    );
    if notice.len() > max_output_bytes {
        return Err(ToolError::new(
            ToolErrorKind::Execution,
            format!(
                "workflow tool output budget is too small: accepted limit {max_output_bytes}, required {}",
                notice.len()
            ),
        ));
    }
    if notice.len() == max_output_bytes {
        return Ok(notice);
    }

    let prefix_bytes = max_output_bytes - notice.len() - 1;
    let candidate = byte_prefix(&formatted, prefix_bytes);
    let Some(boundary) = candidate.rfind('\n') else {
        return Ok(notice);
    };
    if boundary == 0 {
        return Ok(notice);
    }
    Ok(format!("{}\n{notice}", &candidate[..boundary]))
}

fn byte_prefix(value: &str, max_bytes: usize) -> &str {
    let mut boundary = max_bytes.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}
