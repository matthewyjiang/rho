use crate::workflow::{
    apply_event, AttemptNumber, NodeId, SchedulerEvent, StoredRun, WorkflowEvent, WorkflowStore,
};

use super::RuntimeError;

pub(super) fn replay_journal(
    store: &WorkflowStore,
    run_directory: &std::path::Path,
    run: &mut StoredRun,
) -> Result<bool, RuntimeError> {
    let events = store.read_events(run.manifest.run_id)?;
    let mut changed = false;
    let mut pending_outputs = std::collections::BTreeMap::new();
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
        match record.event {
            WorkflowEvent::NodeReady { node } => {
                run.state.state = apply_event(
                    &run.graph,
                    &run.state.state,
                    SchedulerEvent::MarkReady { node },
                )?;
            }
            WorkflowEvent::LaunchIntended { .. } => {}
            WorkflowEvent::AttemptStarted { node, attempt, .. } => {
                run.state.state = apply_event(
                    &run.graph,
                    &run.state.state,
                    SchedulerEvent::Launched { node, attempt },
                )?;
            }
            WorkflowEvent::NodeFinished {
                node,
                attempt,
                outcome,
            } => {
                let output = match pending_outputs.remove(&node) {
                    Some(output) => Some(output),
                    None => read_attempt_output(run_directory, &node, attempt)?,
                };
                let command_exit = read_command_exit(run_directory, &node, attempt)?;
                run.state.state = apply_event(
                    &run.graph,
                    &run.state.state,
                    SchedulerEvent::Finished {
                        node,
                        outcome,
                        command_exit,
                        output,
                    },
                )?;
            }
            WorkflowEvent::StructuredOutput { node, value } => {
                pending_outputs.insert(node, value);
            }
            WorkflowEvent::CancellationRequested => {
                run.state.state = apply_event(
                    &run.graph,
                    &run.state.state,
                    SchedulerEvent::CancellationRequested,
                )?;
            }
            WorkflowEvent::RunLifecycle { lifecycle } => {
                run.state.state.lifecycle = lifecycle;
                run.state.state.revision =
                    run.state.state.revision.checked_add(1).ok_or_else(|| {
                        RuntimeError::Data("workflow state revision overflow".into())
                    })?;
            }
            WorkflowEvent::HookObserved { .. } => {}
        }
        run.state.last_event_sequence = record.sequence;
        changed = true;
    }
    Ok(changed)
}

fn read_attempt_output(
    run_directory: &std::path::Path,
    node: &NodeId,
    attempt: AttemptNumber,
) -> Result<Option<crate::workflow::WorkflowValue>, RuntimeError> {
    let path = run_directory
        .join("nodes")
        .join(node.as_str())
        .join("attempts")
        .join(attempt.to_string())
        .join("output.json");
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&std::fs::read(path)?)?))
}

fn read_command_exit(
    run_directory: &std::path::Path,
    node: &NodeId,
    attempt: AttemptNumber,
) -> Result<Option<crate::workflow::CommandExit>, RuntimeError> {
    let path = run_directory
        .join("nodes")
        .join(node.as_str())
        .join("attempts")
        .join(attempt.to_string())
        .join("command.json");
    if !path.is_file() {
        return Ok(None);
    }
    let outcome: crate::workflow::CommandOutcome = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(Some(outcome.exit))
}
