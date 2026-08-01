use crate::workflow::{
    apply_event, validate_lifecycle_transition, AttemptNumber, AttemptRecord, AttemptState,
    NodeCompletion, NodeId, NodeResetReason, NodeTerminalState, SchedulerEvent, StoredRun,
    WorkflowEvent, WorkflowState, WorkflowStore, ATTEMPT_VERSION,
};

use super::RuntimeError;

pub(super) fn replay_journal(
    store: &WorkflowStore,
    run_directory: &std::path::Path,
    run: &mut StoredRun,
) -> Result<bool, RuntimeError> {
    let events = store.read_events(run.manifest.run_id)?;
    let mut changed = false;
    let snapshot_sequence = run.state.last_event_sequence;
    for record in events
        .into_iter()
        .filter(|event| event.sequence > snapshot_sequence)
    {
        let expected = run
            .state
            .last_event_sequence
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Data("workflow event sequence overflow".into()))?;
        if record.sequence != expected {
            return Err(RuntimeError::Data(format!(
                "workflow journal sequence {} followed {}",
                record.sequence, run.state.last_event_sequence
            )));
        }
        run.state.state =
            apply_durable_event(&run.graph, run_directory, &run.state.state, &record.event)?;
        run.state.last_event_sequence = record.sequence;
        changed = true;
    }
    Ok(changed)
}

pub(super) fn apply_durable_event(
    graph: &crate::workflow::FrozenWorkflow,
    _run_directory: &std::path::Path,
    state: &WorkflowState,
    event: &WorkflowEvent,
) -> Result<WorkflowState, RuntimeError> {
    Ok(match event {
        WorkflowEvent::NodeReady { node } => apply_event(
            graph,
            state,
            SchedulerEvent::MarkReady { node: node.clone() },
        )?,
        WorkflowEvent::LaunchIntended { .. }
        | WorkflowEvent::StructuredOutput { .. }
        | WorkflowEvent::HookObserved { .. }
        | WorkflowEvent::CancellationAcknowledged { .. } => state.clone(),
        WorkflowEvent::AttemptStarted { node, attempt, .. } => apply_event(
            graph,
            state,
            SchedulerEvent::Launched {
                node: node.clone(),
                attempt: *attempt,
            },
        )?,
        WorkflowEvent::NodeFinished { node, completion } => {
            let completion = match completion.attempt {
                Some(_) => completion.as_ref().clone(),
                None if completion.outcome == NodeTerminalState::Cancellation => {
                    let resume = match state.nodes.get(node) {
                        Some(crate::workflow::NodeState::Pending) => {
                            crate::workflow::CancellationResumeState::Pending
                        }
                        Some(crate::workflow::NodeState::Ready) => {
                            crate::workflow::CancellationResumeState::Ready
                        }
                        _ => {
                            return Err(RuntimeError::Data(format!(
                                "synthetic cancellation targets non-waiting node '{node}'"
                            )))
                        }
                    };
                    NodeCompletion::cancelled(resume)
                }
                None => completion.as_ref().clone(),
            };
            apply_event(
                graph,
                state,
                SchedulerEvent::Finished {
                    node: node.clone(),
                    completion: Box::new(completion),
                },
            )?
        }
        WorkflowEvent::CancellationRequested { .. } => {
            apply_event(graph, state, SchedulerEvent::CancellationRequested)?
        }
        WorkflowEvent::NodeReset { node, reason } => apply_event(
            graph,
            state,
            SchedulerEvent::ResetNode {
                node: node.clone(),
                reason: *reason,
            },
        )?,
        WorkflowEvent::CancellationCleared => {
            let mut next = state.clone();
            if !next.cancellation_requested {
                return Err(RuntimeError::Data(
                    "cancellation clear event has no cancellation to clear".into(),
                ));
            }
            next.cancellation_requested = false;
            bump_revision(&mut next)?;
            next
        }
        WorkflowEvent::RunLifecycle { lifecycle } => {
            let outcome = validate_lifecycle_transition(graph, state, *lifecycle)?;
            let mut next = state.clone();
            next.lifecycle = *lifecycle;
            next.outcome = outcome;
            bump_revision(&mut next)?;
            next
        }
    })
}

pub(super) fn completed_attempt(
    run_directory: &std::path::Path,
    node: &NodeId,
    attempt: AttemptNumber,
) -> Result<Option<NodeCompletion>, RuntimeError> {
    let record = read_attempt_record(run_directory, node, attempt)?;
    Ok(match record.state {
        AttemptState::Completed { completion } => Some(*completion),
        AttemptState::LaunchIntended
        | AttemptState::Started { .. }
        | AttemptState::CleanlyCancelled
        | AttemptState::InterruptedUncertain { .. } => None,
    })
}

pub(super) fn read_attempt_record(
    run_directory: &std::path::Path,
    node: &NodeId,
    attempt: AttemptNumber,
) -> Result<AttemptRecord, RuntimeError> {
    let path = attempt_status_path(run_directory, node, attempt);
    let relative = path
        .strip_prefix(run_directory)
        .map_err(|_| RuntimeError::UnsafeArtifact(path.clone()))?;
    let mut file = crate::workflow::open_private_file_beneath(run_directory, relative, false)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)?;
    let record: AttemptRecord = serde_json::from_slice(&bytes)?;
    crate::workflow::check_schema_version(
        "workflow attempt",
        record.schema_version,
        ATTEMPT_VERSION,
    )?;
    if record.attempt != attempt {
        return Err(RuntimeError::Data(format!(
            "attempt record for node '{node}' has the wrong attempt number"
        )));
    }
    Ok(record)
}

fn attempt_status_path(
    run_directory: &std::path::Path,
    node: &NodeId,
    attempt: AttemptNumber,
) -> std::path::PathBuf {
    run_directory
        .join("nodes")
        .join(node.as_str())
        .join("attempts")
        .join(attempt.to_string())
        .join("status.json")
}

fn bump_revision(state: &mut WorkflowState) -> Result<(), RuntimeError> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Data("workflow state revision overflow".into()))?;
    Ok(())
}

pub(super) fn reset_event(node: NodeId, reason: NodeResetReason) -> WorkflowEvent {
    WorkflowEvent::NodeReset { node, reason }
}
