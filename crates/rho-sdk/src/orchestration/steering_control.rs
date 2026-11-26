use std::future::Future;

use tokio::sync::mpsc;

use crate::{
    provider::ProviderSteeringOutcome, run::RunCommand, steering::SteeringQueue, CancellationToken,
    Error, RunEvent,
};

use super::emit;

pub(in crate::orchestration) async fn handle_outcome(
    (id, outcome): (crate::SteeringId, ProviderSteeringOutcome),
    steering: &mut SteeringQueue,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), Error> {
    match outcome {
        ProviderSteeringOutcome::Accepted => {
            steering.mark_delivered(&id);
            emit(events, cancellation, RunEvent::SteeringDelivered { id }).await
        }
        ProviderSteeringOutcome::Released => {
            steering.mark_released(&id);
            Ok(())
        }
    }
}

pub(in crate::orchestration) async fn apply_staged(
    steering: &mut SteeringQueue,
    history: &mut Vec<crate::model::Message>,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), Error> {
    let ids = steering.planned_apply_ids();
    if ids.is_empty() {
        return Ok(());
    }
    // Publish before mutating history so cancellation cannot hide applied IDs from hosts.
    // There is deliberately no await between successful publication and the mutation.
    emit(events, cancellation, RunEvent::SteeringApplied { ids }).await?;
    steering.apply(history);
    Ok(())
}

pub(in crate::orchestration) fn accept_command(command: RunCommand, steering: &mut SteeringQueue) {
    match command {
        RunCommand::Steer { input, accepted } => {
            let id = steering.accept(input);
            let _ = accepted.send(id);
        }
        RunCommand::RetractSteering { id, completed } => {
            let _ = completed.send(steering.retract(&id));
        }
        RunCommand::Respond { accepted, .. } => {
            let _ = accepted.send(Err("no host input request is awaiting a response".into()));
        }
    }
}

/// Awaits `work` while accepting the run's commands, so a steer or a
/// withdrawal answers at once even while the run waits on something that
/// takes no commands of its own: the host holding a checkpoint open for
/// background work the turn waits for, a compaction, or a retry delay. A
/// provider request and a tool batch accept commands themselves.
pub(in crate::orchestration) async fn accepting_commands<T>(
    commands: &mut mpsc::Receiver<RunCommand>,
    steering: &mut SteeringQueue,
    work: impl Future<Output = T>,
) -> T {
    tokio::pin!(work);
    let mut commands_open = true;
    loop {
        tokio::select! {
            biased;
            output = &mut work => return output,
            command = commands.recv(), if commands_open => match command {
                Some(command) => accept_command(command, steering),
                None => commands_open = false,
            },
        }
    }
}

pub(in crate::orchestration) fn drain_commands(
    commands: &mut mpsc::Receiver<RunCommand>,
    steering: &mut SteeringQueue,
) {
    while let Ok(command) = commands.try_recv() {
        accept_command(command, steering);
    }
}
