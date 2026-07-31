use crate::workflow::{
    FrozenWorkflow, NodeId, NodeResetReason, NodeState, NodeTerminalState, RunLifecycle,
    RunMutationGuard, RunStateRecord, WorkflowEvent, WorkflowStore,
};

use super::{journal::reset_event, runner::persist_state_event, RecoveryDecision, RuntimeError};

pub(super) fn uncertain_nodes(state: &RunStateRecord) -> Vec<NodeId> {
    state
        .state
        .nodes
        .iter()
        .filter_map(|(node, value)| {
            matches!(value, NodeState::Running { .. }).then_some(node.clone())
        })
        .collect()
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
    if uncertain.is_empty() && state.state.lifecycle != RunLifecycle::NeedsRecovery {
        if state.state.lifecycle != RunLifecycle::Planned
            && state.state.lifecycle != RunLifecycle::Running
            && (state.state.cancellation_requested
                || state.state.nodes.values().any(|node| {
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
        .nodes
        .iter()
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
