use crate::workflow::{
    attempt_directory, AttemptNumber, AttemptRecord, AttemptState, ExternalOwner, NodeResetReason,
    NodeState, NodeTerminalState, RunLifecycle, RunStateRecord, TaskInstanceId, WorkflowEvent,
    ATTEMPT_VERSION,
};

use super::{
    artifacts::write_json,
    cancellation::read_cancellation_request,
    journal::{completed_attempt, read_attempt_record, RunJournal},
    runner::send_event,
    RecoveryDecision, RuntimeError, RuntimeEvent,
};

/// Compute resume policy once, after replay and recovery of completed attempts.
pub(super) enum ResumePlan {
    Finished,
    FinishClosedScope,
    Resume {
        reopen_root: bool,
        cancelled: Vec<TaskInstanceId>,
        uncertain: Vec<TaskInstanceId>,
        needs_confirmation: bool,
    },
}

impl ResumePlan {
    pub(super) fn for_run(record: &RunStateRecord) -> Self {
        let state = &record.state;
        let cancelled = cancelled_nodes(record);
        let reopen_root = state
            .run_result()
            .is_some_and(|result| result.outcome == crate::workflow::WorkflowOutcome::Cancellation);
        if state.run_result().is_some() && !reopen_root {
            return if state.lifecycle == RunLifecycle::Completed && !state.cancellation_requested {
                Self::Finished
            } else {
                Self::FinishClosedScope
            };
        }
        let uncertain = uncertain_nodes(record);
        let needs_confirmation =
            !uncertain.is_empty() || state.lifecycle == RunLifecycle::NeedsRecovery;
        Self::Resume {
            reopen_root,
            cancelled,
            uncertain,
            needs_confirmation,
        }
    }
}

fn cancelled_nodes(record: &RunStateRecord) -> Vec<TaskInstanceId> {
    record
        .state
        .tasks()
        .filter_map(|(node, state)| {
            matches!(
                state,
                NodeState::Terminal {
                    outcome: NodeTerminalState::Cancellation
                }
            )
            .then_some(node)
        })
        .collect()
}

fn uncertain_nodes(state: &RunStateRecord) -> Vec<TaskInstanceId> {
    state
        .state
        .tasks()
        .filter_map(|(node, value)| matches!(value, NodeState::Running { .. }).then_some(node))
        .collect()
}

fn mark_uncertain_attempts(
    run_directory: &std::path::Path,
    state: &RunStateRecord,
) -> Result<(), RuntimeError> {
    for (node, node_state) in state.state.tasks() {
        if let NodeState::Running { attempt } = node_state {
            mark_attempt_uncertain(run_directory, &node, *attempt)?;
        }
    }
    Ok(())
}

pub(super) fn mark_attempt_uncertain(
    run_directory: &std::path::Path,
    node: &TaskInstanceId,
    attempt: AttemptNumber,
) -> Result<(), RuntimeError> {
    let record = read_attempt_record(run_directory, node, attempt)?;
    let owner = match record.state {
        AttemptState::Started { owner } | AttemptState::InterruptedUncertain { owner } => owner,
        AttemptState::LaunchIntended => ExternalOwner::Process { pid: 0 },
        AttemptState::Completed { .. } | AttemptState::CleanlyCancelled => {
            return Err(RuntimeError::Data(format!(
                "node '{node}' cleanup became uncertain after its attempt was terminal"
            )))
        }
    };
    let attempt_directory = attempt_directory(run_directory, node, attempt);
    write_json(
        run_directory,
        &attempt_directory.join("status.json"),
        &AttemptRecord {
            schema_version: ATTEMPT_VERSION,
            attempt,
            state: AttemptState::InterruptedUncertain { owner },
        },
    )
    .map(|_| ())
}

/// Execute the precomputed policy. Confirmation precedes resets and acknowledgement:
/// no interrupted external process is ever silently assumed to have exited.
pub(super) fn recover_state(
    journal: &mut RunJournal,
    plan: ResumePlan,
    decision: RecoveryDecision,
    known_cancellation: Option<&str>,
    pending_cancellation: Option<String>,
    events: &Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
) -> Result<(), RuntimeError> {
    if let ResumePlan::Resume {
        uncertain,
        needs_confirmation,
        ..
    } = &plan
    {
        if !uncertain.is_empty() {
            mark_uncertain_attempts(&journal.directory, &journal.run.state)?;
            journal.commit(WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::NeedsRecovery,
            })?;
            send_event(
                events,
                RuntimeEvent::NeedsRecovery {
                    nodes: uncertain.clone(),
                },
            );
        }
        if *needs_confirmation && decision != RecoveryDecision::ConfirmNoProcess {
            return Err(RuntimeError::NeedsRecovery {
                nodes: uncertain
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    }
    if journal.run.state.state.cancellation_requested {
        if let Some(request_id) = pending_cancellation {
            journal.commit(WorkflowEvent::CancellationAcknowledged { request_id })?;
        }
    }
    match plan {
        ResumePlan::Finished => return Ok(()),
        ResumePlan::FinishClosedScope => {}
        ResumePlan::Resume {
            reopen_root,
            cancelled,
            uncertain,
            ..
        } => {
            if reopen_root {
                journal.commit(WorkflowEvent::ScopeReopened {
                    scope: crate::workflow::ScopeInstanceId::ROOT,
                })?;
            }
            if journal.run.state.state.lifecycle != RunLifecycle::Running {
                journal.commit(WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::Running,
                })?;
            }
            for node in uncertain {
                journal.commit(WorkflowEvent::NodeReset {
                    node,
                    reason: NodeResetReason::InterruptedRecovery,
                })?;
            }
            for node in cancelled {
                journal.commit(WorkflowEvent::NodeReset {
                    node,
                    reason: NodeResetReason::CleanCancellation,
                })?;
            }
        }
    }
    // Remove the acknowledged request before clearing durable cancellation. A
    // crash in between can repeat this cleanup; the opposite ordering can feed
    // a stale request back into the resumed driver. Never remove a newer receipt
    // installed after the old file was removed (including across a crash).
    let run_id = journal.run.manifest.run_id;
    if let Some(request_id) = known_cancellation {
        if read_cancellation_request(&journal.store, run_id)?.as_deref() == Some(request_id) {
            journal.store.clear_cancellation_request(run_id)?;
        }
    }
    if journal.run.state.state.cancellation_requested {
        journal.commit(WorkflowEvent::CancellationCleared)?;
    }
    Ok(())
}

pub(super) fn recover_completed_transitions(journal: &mut RunJournal) -> Result<(), RuntimeError> {
    let running = journal
        .run
        .state
        .state
        .tasks()
        .filter_map(|(node, state)| match state {
            NodeState::Running { attempt } => Some((node, *attempt)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (node, attempt) in running {
        let Some(completion) = completed_attempt(&journal.directory, &node, attempt)? else {
            continue;
        };
        if let Some(output) = completion.structured_output.clone() {
            if journal
                .run
                .state
                .state
                .structured_output(&node, attempt)
                .is_none()
            {
                journal.commit(WorkflowEvent::StructuredOutput {
                    node: node.clone(),
                    attempt,
                    output,
                })?;
            }
        }
        journal.commit(WorkflowEvent::NodeFinished {
            node,
            completion: Box::new(completion),
        })?;
    }
    Ok(())
}
