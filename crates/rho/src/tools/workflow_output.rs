//! Readable line summaries for model-facing workflow tool results.
//!
//! Formatting produces lines and bounding consumes lines, so an oversized
//! result loses whole trailing lines and clips the last one it keeps, rather
//! than dropping content the budget could still have carried.

use rho_sdk::tool::{ToolError, ToolErrorKind};
use rho_tools::tool::{truncate, TRUNCATION_MARKER};

use super::workflow::{
    WorkflowArtifactSummary, WorkflowDiagnosticSummary, WorkflowNodeSummary, WorkflowToolResult,
};

pub(super) fn format_workflow_result(result: &WorkflowToolResult) -> Vec<String> {
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
                push_diagnostic(&mut lines, diagnostic);
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
        } => {
            lines.push(format!("workflow {run_id}: {}", state.as_str()));
            lines.push(format!("graph_digest: {graph_digest}"));
            lines.push(format!("nodes: {}", nodes.len()));
            for node in nodes {
                push_node(&mut lines, node);
            }
        }
        WorkflowToolResult::Cancel {
            run_id,
            request_id,
            cancellation_state,
            state,
        } => {
            lines.push(format!("workflow {run_id}: {}", state.as_str()));
            lines.push(format!("cancellation: {}", cancellation_state.as_str()));
            if let Some(request_id) = request_id {
                lines.push(format!("request_id: {request_id}"));
            }
        }
    }
    lines
}

fn push_diagnostic(lines: &mut Vec<String>, diagnostic: &WorkflowDiagnosticSummary) {
    let label = format!("{} [{}]", diagnostic.severity, diagnostic.code);
    push_labeled_lines(lines, "  ", &label, &diagnostic.message);
    if let Some(source) = &diagnostic.source {
        push_labeled_lines(lines, "    ", "source", source);
    }
    if let Some(line) = diagnostic.line {
        lines.push(format!("    line: {line}"));
    }
    if let Some(column) = diagnostic.column {
        lines.push(format!("    column: {column}"));
    }
}

fn push_node(lines: &mut Vec<String>, node: &WorkflowNodeSummary) {
    let attempt = node
        .attempt
        .map(|attempt| format!(" · attempt {attempt}"))
        .unwrap_or_default();
    lines.push(format!(
        "  {} · {}{attempt}",
        node.node_id,
        node.state.as_str()
    ));
    lines.extend(node.artifacts.iter().map(artifact_line));
}

fn artifact_line(artifact: &WorkflowArtifactSummary) -> String {
    let reference = &artifact.artifact;
    let observation = reference
        .observation_notice()
        .map(|notice| format!(" · {notice}"))
        .unwrap_or_default();
    format!(
        "    {}: {} · {} bytes · digest {}{observation}",
        artifact.kind.label(),
        reference.relative_path,
        reference.retained_bytes,
        reference.digest.0
    )
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

pub(super) fn bounded_result(
    result: &WorkflowToolResult,
    max_output_bytes: usize,
) -> Result<String, ToolError> {
    let lines = format_workflow_result(result);
    let total_bytes = joined_len(&lines);
    if total_bytes <= max_output_bytes {
        return Ok(lines.join("\n"));
    }

    // Reserve against the widest notice this result can produce. The real
    // notice reports how many lines were dropped, which is never more than the
    // line count, so it can only be shorter than the reservation.
    let reserved = omission_notice(lines.len(), total_bytes, max_output_bytes).len();
    if reserved > max_output_bytes {
        return Err(ToolError::new(
            ToolErrorKind::Execution,
            format!(
                "workflow tool output budget is too small: accepted limit {max_output_bytes}, required {reserved}"
            ),
        ));
    }
    let Some(budget) = max_output_bytes.checked_sub(reserved + 1) else {
        return Ok(omission_notice(lines.len(), total_bytes, max_output_bytes));
    };

    let mut kept = Vec::new();
    let mut delivered = 0;
    let mut used = 0;
    for line in &lines {
        let separator = usize::from(!kept.is_empty());
        if used + separator + line.len() <= budget {
            used += separator + line.len();
            delivered += 1;
            kept.push(line.clone());
            continue;
        }
        // A line longer than the whole budget could never be delivered intact,
        // so clip it and keep what fits; one huge diagnostic still reports
        // which diagnostic it was. A line that merely ran out of room here is
        // dropped whole, because a two-character stub tells the model nothing.
        let available = budget.saturating_sub(used + separator);
        if line.len() > budget && available > TRUNCATION_MARKER.len() {
            kept.push(truncate(line.clone(), available - TRUNCATION_MARKER.len()));
        }
        break;
    }

    kept.push(omission_notice(
        lines.len() - delivered,
        total_bytes,
        max_output_bytes,
    ));
    Ok(kept.join("\n"))
}

/// Bytes the lines occupy once newline-joined.
fn joined_len(lines: &[String]) -> usize {
    lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1)
}

fn omission_notice(omitted: usize, total_bytes: usize, max_output_bytes: usize) -> String {
    format!(
        "... {omitted} more line(s) omitted; workflow summary is {total_bytes} bytes and the limit is {max_output_bytes} bytes"
    )
}

#[cfg(test)]
#[path = "workflow_output_tests.rs"]
mod tests;
