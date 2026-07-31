use std::path::Path;

use super::{
    apply_event, validate_lifecycle_transition, CancellationResumeState, FrozenWorkflow,
    NodeCompletion, NodeId, NodeState, NodeTerminalState, RunLifecycle, SchedulerEvent,
    WorkflowError, WorkflowEvent, WorkflowEventRecord, WorkflowResult, WorkflowState,
};

pub(super) fn derive_snapshot(
    graph: &FrozenWorkflow,
    events: &[WorkflowEventRecord],
    through: u64,
    path: &Path,
) -> WorkflowResult<WorkflowState> {
    let mut state = WorkflowState {
        revision: 0,
        lifecycle: RunLifecycle::Planned,
        outcome: None,
        cancellation_requested: false,
        nodes: graph
            .graph
            .nodes
            .keys()
            .cloned()
            .map(|node| (node, NodeState::Pending))
            .collect(),
        command_exits: std::collections::BTreeMap::new(),
        outputs: std::collections::BTreeMap::new(),
        completions: std::collections::BTreeMap::new(),
    };
    let mut structured = std::collections::BTreeMap::new();
    for record in events
        .iter()
        .take_while(|record| record.sequence <= through)
    {
        state = match &record.event {
            WorkflowEvent::NodeReady { node } => apply_event(
                graph,
                &state,
                SchedulerEvent::MarkReady { node: node.clone() },
            )?,
            WorkflowEvent::AttemptStarted { node, attempt, .. } => apply_event(
                graph,
                &state,
                SchedulerEvent::Launched {
                    node: node.clone(),
                    attempt: *attempt,
                },
            )?,
            WorkflowEvent::StructuredOutput {
                node,
                attempt,
                output,
            } => {
                if state.nodes.get(node) != Some(&NodeState::Running { attempt: *attempt })
                    || structured
                        .insert((node.clone(), *attempt), output.clone())
                        .is_some()
                {
                    return corrupt(path, "structured output event has no unique active attempt");
                }
                state
            }
            WorkflowEvent::NodeFinished { node, completion } => {
                let completion = event_completion(&state, node, completion, path)?;
                if let Some(attempt) = completion.attempt {
                    let recorded = structured.remove(&(node.clone(), attempt));
                    if recorded.as_ref() != completion.structured_output.as_ref() {
                        return corrupt(
                            path,
                            "structured output event differs from node completion",
                        );
                    }
                }
                apply_event(
                    graph,
                    &state,
                    SchedulerEvent::Finished {
                        node: node.clone(),
                        completion: Box::new(completion),
                    },
                )?
            }
            WorkflowEvent::CancellationRequested { .. } => {
                apply_event(graph, &state, SchedulerEvent::CancellationRequested)?
            }
            WorkflowEvent::NodeReset { node, reason } => apply_event(
                graph,
                &state,
                SchedulerEvent::ResetNode {
                    node: node.clone(),
                    reason: *reason,
                },
            )?,
            WorkflowEvent::CancellationCleared => {
                if !state.cancellation_requested {
                    return corrupt(path, "cancellation clear event has no cancellation");
                }
                let mut next = state;
                next.cancellation_requested = false;
                bump_revision(&mut next, path)?;
                next
            }
            WorkflowEvent::RunLifecycle { lifecycle } => {
                let outcome = validate_lifecycle_transition(graph, &state, *lifecycle)?;
                let mut next = state;
                next.lifecycle = *lifecycle;
                next.outcome = outcome;
                bump_revision(&mut next, path)?;
                next
            }
            WorkflowEvent::LaunchIntended { .. }
            | WorkflowEvent::CancellationAcknowledged { .. }
            | WorkflowEvent::HookObserved { .. } => state,
        };
    }
    Ok(state)
}

fn event_completion(
    state: &WorkflowState,
    node: &NodeId,
    completion: &NodeCompletion,
    path: &Path,
) -> WorkflowResult<NodeCompletion> {
    match completion.attempt {
        Some(attempt) if state.nodes.get(node) == Some(&NodeState::Running { attempt }) => {
            Ok(completion.clone())
        }
        Some(_) => corrupt(path, "node completion does not match its active attempt"),
        None if completion.outcome == NodeTerminalState::Cancellation => {
            let resume = match state.nodes.get(node) {
                Some(NodeState::Pending) => CancellationResumeState::Pending,
                Some(NodeState::Ready) => CancellationResumeState::Ready,
                _ => return corrupt(path, "synthetic cancellation targets a non-waiting node"),
            };
            if completion.command_exit.is_some()
                || completion.structured_output.is_some()
                || completion.artifacts.iter().next().is_some()
            {
                return corrupt(path, "synthetic cancellation contains attempt-owned data");
            }
            Ok(NodeCompletion::cancelled(resume))
        }
        None => Ok(completion.clone()),
    }
}

fn bump_revision(state: &mut WorkflowState, path: &Path) -> WorkflowResult<()> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| WorkflowError::Corrupt {
            path: path.to_path_buf(),
            reason: "workflow state revision overflowed".to_owned(),
        })?;
    Ok(())
}

fn corrupt<T>(path: &Path, reason: &str) -> WorkflowResult<T> {
    Err(WorkflowError::Corrupt {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    })
}
