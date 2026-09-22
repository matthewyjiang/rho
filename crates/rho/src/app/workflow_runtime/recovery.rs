use crate::workflow::{
    attempt_directory, AttemptNumber, AttemptRecord, AttemptState, ExternalOwner, FrozenWorkflow,
    NodeResetReason, NodeState, NodeTerminalState, RunLifecycle, RunMutationGuard, RunStateRecord,
    TaskInstanceId, WorkflowEvent, WorkflowStore, ATTEMPT_VERSION,
};

use super::{
    artifacts::write_json,
    journal::{read_attempt_record, reset_event},
    runner::persist_state_event,
    RecoveryDecision, RuntimeError,
};

pub(super) fn uncertain_nodes(state: &RunStateRecord) -> Vec<TaskInstanceId> {
    state
        .state
        .tasks()
        .filter_map(|(node, value)| {
            matches!(value, NodeState::Running { .. }).then_some(node.clone())
        })
        .collect()
}

pub(super) fn mark_uncertain_attempts(
    run_directory: &std::path::Path,
    state: &RunStateRecord,
) -> Result<(), RuntimeError> {
    for (node, node_state) in state.state.tasks() {
        let NodeState::Running { attempt } = node_state else {
            continue;
        };
        mark_attempt_uncertain(run_directory, &node, *attempt)?;
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

pub(super) fn recover_state(
    store: &WorkflowStore,
    guard: &mut RunMutationGuard,
    run_directory: &std::path::Path,
    graph: &FrozenWorkflow,
    state: &mut RunStateRecord,
    decision: RecoveryDecision,
) -> Result<(), RuntimeError> {
    let uncertain = uncertain_nodes(state);
    if state
        .state
        .root_scope()
        .result
        .as_ref()
        .is_some_and(|result| result.outcome == crate::workflow::WorkflowOutcome::Cancellation)
    {
        persist_state_event(
            store,
            guard,
            run_directory,
            graph,
            state,
            WorkflowEvent::ScopeReopened {
                scope: crate::workflow::ScopeInstanceId::ROOT,
            },
        )?;
    }
    if state.state.run_result().is_some() {
        // A late cancellation can arrive after every task completed, so a closed
        // scope need not have a cancellation outcome. Preserve that result and
        // let the driver finish the run instead of reopening successful work.
        if state.state.cancellation_requested {
            persist_state_event(
                store,
                guard,
                run_directory,
                graph,
                state,
                WorkflowEvent::CancellationCleared,
            )?;
        }
        return Ok(());
    }
    if uncertain.is_empty() && state.state.lifecycle != RunLifecycle::NeedsRecovery {
        if state.state.lifecycle != RunLifecycle::Planned
            && state.state.lifecycle != RunLifecycle::Running
            && (state.state.cancellation_requested
                || state.state.tasks().any(|(_, node)| {
                    matches!(
                        node,
                        NodeState::Terminal {
                            outcome: NodeTerminalState::Cancellation
                        }
                    )
                }))
        {
            persist_state_event(
                store,
                guard,
                run_directory,
                graph,
                state,
                WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::Running,
                },
            )?;
        }
        return reset_clean_cancellations(store, guard, run_directory, graph, state);
    }
    if decision != RecoveryDecision::ConfirmNoProcess {
        return Err(RuntimeError::NeedsRecovery {
            nodes: uncertain
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    if state.state.lifecycle != RunLifecycle::Running {
        persist_state_event(
            store,
            guard,
            run_directory,
            graph,
            state,
            WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::Running,
            },
        )?;
    }
    for node in uncertain {
        persist_state_event(
            store,
            guard,
            run_directory,
            graph,
            state,
            reset_event(node, NodeResetReason::InterruptedRecovery),
        )?;
    }
    reset_clean_cancellations(store, guard, run_directory, graph, state)
}

fn reset_clean_cancellations(
    store: &WorkflowStore,
    guard: &mut RunMutationGuard,
    run_directory: &std::path::Path,
    graph: &FrozenWorkflow,
    state: &mut RunStateRecord,
) -> Result<(), RuntimeError> {
    let cancelled = state
        .state
        .tasks()
        .filter_map(|(node, value)| {
            matches!(
                value,
                NodeState::Terminal {
                    outcome: NodeTerminalState::Cancellation
                }
            )
            .then_some(node.clone())
        })
        .collect::<Vec<_>>();
    for node in cancelled {
        persist_state_event(
            store,
            guard,
            run_directory,
            graph,
            state,
            reset_event(node, NodeResetReason::CleanCancellation),
        )?;
    }
    if state.state.cancellation_requested {
        persist_state_event(
            store,
            guard,
            run_directory,
            graph,
            state,
            WorkflowEvent::CancellationCleared,
        )?;
    }
    Ok(())
}
