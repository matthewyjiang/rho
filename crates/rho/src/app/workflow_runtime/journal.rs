use crate::workflow::{
    apply_durable_event, attempt_directory, AttemptNumber, AttemptRecord, AttemptState,
    NodeCompletion, NodeResetReason, StoredRun, TaskInstanceId, WorkflowEvent, WorkflowStore,
    ATTEMPT_VERSION,
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
            apply_durable_event(&run.graph, &run.state.state, &record.event, run_directory)?;
        run.state.last_event_sequence = record.sequence;
        changed = true;
    }
    Ok(changed)
}

pub(super) fn completed_attempt(
    run_directory: &std::path::Path,
    node: &TaskInstanceId,
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
    node: &TaskInstanceId,
    attempt: AttemptNumber,
) -> Result<AttemptRecord, RuntimeError> {
    let path = attempt_directory(run_directory, node, attempt).join("status.json");
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

pub(super) fn reset_event(node: TaskInstanceId, reason: NodeResetReason) -> WorkflowEvent {
    WorkflowEvent::NodeReset { node, reason }
}
