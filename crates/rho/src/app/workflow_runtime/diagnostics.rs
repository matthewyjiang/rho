//! Persist driver/executor failures through the existing attempt output channels.
use crate::workflow::{
    attempt_directory, ArtifactObservation, AttemptNumber, NodeExecution, NodeTerminalState,
    TaskInstanceId,
};

use super::{
    artifacts::write_artifact_with_observation, journal::RunJournal, NodeExecutionResult,
    NodeProgressReporter, RuntimeError, RuntimeEvent,
};

pub(super) fn failed_attempt(
    journal: &RunJournal,
    node: &TaskInstanceId,
    attempt: AttemptNumber,
    error: &RuntimeError,
    events: &Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
) -> Result<NodeExecutionResult, RuntimeError> {
    let outcome = match error {
        RuntimeError::Denied(_) => NodeTerminalState::Denial,
        RuntimeError::Cancelled => {
            return Ok(NodeExecutionResult::terminal(
                NodeTerminalState::Cancellation,
            ))
        }
        _ => NodeTerminalState::Failure,
    };
    let message = error.to_string();
    if let Some(sender) = events {
        NodeProgressReporter::new(node.clone(), attempt, sender.clone()).message(message.clone());
    }
    let leaf = journal
        .run
        .graph
        .leaf(node)
        .ok_or_else(|| RuntimeError::LaunchMetadata { node: node.clone() })?;
    let limit = usize::try_from(leaf.node.max_output_bytes).unwrap_or(usize::MAX);
    let retained = rho_sdk::floor_char_boundary(&message, limit);
    let observed_bytes = message.len() as u64;
    let artifact = write_artifact_with_observation(
        &journal.directory,
        &attempt_directory(&journal.directory, node, attempt).join("error.txt"),
        &message.as_bytes()[..retained],
        if retained == message.len() {
            ArtifactObservation::Complete { observed_bytes }
        } else {
            ArtifactObservation::Truncated {
                observed_bytes_at_least: observed_bytes,
            }
        },
    )?;
    let mut result = NodeExecutionResult::terminal(outcome);
    match leaf.node.execution {
        NodeExecution::Agent(_) => result.artifacts.answer = Some(artifact),
        NodeExecution::Command(_) => result.artifacts.stderr = Some(artifact),
    }
    Ok(result)
}
