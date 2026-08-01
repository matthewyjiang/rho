//! Project durable stored runs into the workflow TUI snapshot model.
//!
//! Adapters own I/O and event delivery. This module owns the pure mapping so
//! CLI bridges stay thin and tests can lock projection without a runner.

use sha2::{Digest as _, Sha256};

use super::event_adapter::{
    ArtifactReference, CancellationState, ExecutionMetadata, PlanApprovalState,
    SourceDigestSummary, TerminalReason, WorkflowNodeSnapshot, WorkflowSnapshot,
};
use crate::workflow::{
    derive_workflow_outcome, CommandNode, Digest, NodeExecution, NodeId, NodeState,
    NodeTerminalState, ResolvedNode, RunLifecycle, StoredRun, Template, TemplatePart,
    WorkflowState,
};

/// Build a TUI snapshot from a fully loaded durable run.
pub(crate) fn from_stored_run(run: &StoredRun) -> WorkflowSnapshot {
    let state = &run.state.state;
    let nodes = run
        .graph
        .graph
        .nodes
        .iter()
        .map(|(id, node)| {
            let node_state = state.nodes[id].clone();
            let current_attempt = match node_state {
                NodeState::Running { attempt } => Some(attempt),
                _ => None,
            };
            let execution = match &run.graph.resolved_nodes[id] {
                ResolvedNode::Agent(agent) => ExecutionMetadata::Agent {
                    name: agent.agent_id.clone(),
                    runtime: agent.runtime,
                    provider: agent.provider.clone(),
                    model: agent.model.clone(),
                },
                ResolvedNode::Command(command) => ExecutionMetadata::Command {
                    executable: command.executable.clone(),
                    cwd: command.cwd.clone(),
                    shell: matches!(
                        node.execution,
                        NodeExecution::Command(CommandNode::Shell { .. })
                    ),
                },
            };
            WorkflowNodeSnapshot {
                id: id.clone(),
                display_name: node.display_name.clone(),
                dependencies: node.needs.clone(),
                access: node.access,
                execution,
                work: node_work_summary(&node.execution),
                state: node_state.clone(),
                current_attempt,
                command_exit: state.command_exits.get(id).cloned(),
                validated_output: state.outputs.get(id).cloned(),
                artifacts: durable_artifacts_for_node(state, id),
                terminal_reason: terminal_reason(&node_state),
            }
        })
        .collect();
    let lifecycle = state.lifecycle;
    WorkflowSnapshot {
        workflow_name: run.graph.graph.name.to_string(),
        plan_id: run.manifest.plan_id,
        run_id: Some(run.manifest.run_id),
        graph_digest: run.manifest.graph_digest.clone(),
        sources: SourceDigestSummary {
            source_count: run.graph.sources.modules.len(),
            digest: source_digest(run),
        },
        approval: PlanApprovalState::Approved,
        lifecycle,
        outcome: derive_workflow_outcome(&run.graph, state),
        nodes,
        cancellation: if state.cancellation_requested {
            if lifecycle == RunLifecycle::Completed {
                CancellationState::Saved
            } else {
                CancellationState::Requested
            }
        } else {
            CancellationState::NotRequested
        },
        recovery_requirement: None,
    }
}

pub(crate) fn durable_artifacts_for_node(
    state: &WorkflowState,
    id: &NodeId,
) -> Vec<ArtifactReference> {
    state
        .completions
        .get(id)
        .into_iter()
        .flat_map(|completion| completion.artifacts.iter())
        .map(|(kind, artifact)| ArtifactReference {
            kind,
            artifact: artifact.clone(),
        })
        .collect()
}

fn node_work_summary(execution: &NodeExecution) -> String {
    match execution {
        NodeExecution::Agent(agent) => {
            let preview = template_preview(&agent.prompt);
            if preview.is_empty() {
                format!("agent {}", agent.agent)
            } else {
                preview
            }
        }
        NodeExecution::Command(CommandNode::Direct {
            executable,
            arguments,
            ..
        }) => {
            let exe = std::path::Path::new(executable)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(executable);
            if arguments.is_empty() {
                format!("run {exe}")
            } else {
                let args = arguments
                    .iter()
                    .map(template_preview)
                    .collect::<Vec<_>>()
                    .join(" ");
                truncate_chars(&format!("run {exe} {args}"), 160)
            }
        }
        NodeExecution::Command(CommandNode::Shell { command, .. }) => {
            truncate_chars(&format!("shell: {command}"), 160)
        }
    }
}

fn template_preview(template: &Template) -> String {
    let mut out = String::new();
    for part in &template.0 {
        match part {
            TemplatePart::Literal { value } => out.push_str(value),
            TemplatePart::Output { reference } => {
                let path = if reference.path.0.is_empty() {
                    String::new()
                } else {
                    format!(".{}", reference.path.0.join("."))
                };
                out.push_str(&format!("{{{{{node}{path}}}}}", node = reference.node));
            }
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, 160)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn source_digest(run: &StoredRun) -> Digest {
    let mut hash = Sha256::new();
    for (label, source) in &run.graph.sources.modules {
        hash.update(label.as_bytes());
        hash.update([0]);
        hash.update(source.digest.0.as_bytes());
        hash.update([0]);
    }
    Digest(format!("sha256:{:x}", hash.finalize()))
}

fn terminal_reason(state: &NodeState) -> Option<TerminalReason> {
    let NodeState::Terminal { outcome } = state else {
        return None;
    };
    match outcome {
        NodeTerminalState::Success | NodeTerminalState::Skipped => None,
        NodeTerminalState::Failure => Some(TerminalReason::Failure("node failed".into())),
        NodeTerminalState::Denial => Some(TerminalReason::Denial("node was denied".into())),
        NodeTerminalState::Cancellation => {
            Some(TerminalReason::Cancellation("node was cancelled".into()))
        }
        NodeTerminalState::Blocked => Some(TerminalReason::Blocked("node was blocked".into())),
    }
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
