use super::{
    FrozenWorkflow, NodeId, NodeResetReason, NodeState, NodeTerminalState, RunLifecycle,
    WorkflowError, WorkflowOutcome, WorkflowResult, WorkflowState,
};

pub(crate) fn validate_transition(
    node: &NodeId,
    from: &NodeState,
    to: &NodeState,
) -> WorkflowResult<()> {
    let allowed = matches!(
        (from, to),
        (NodeState::Pending, NodeState::Ready)
            | (
                NodeState::Pending,
                NodeState::Terminal {
                    outcome: NodeTerminalState::Skipped
                        | NodeTerminalState::Blocked
                        | NodeTerminalState::Cancellation
                }
            )
            | (NodeState::Ready, NodeState::Running { .. })
            | (
                NodeState::Ready,
                NodeState::Terminal {
                    outcome: NodeTerminalState::Cancellation
                }
            )
            | (NodeState::Running { .. }, NodeState::Terminal { .. })
    );
    if allowed {
        Ok(())
    } else {
        Err(WorkflowError::IllegalTransition {
            node: node.clone(),
            from: format!("{from:?}"),
            to: format!("{to:?}"),
        })
    }
}

pub(crate) fn validate_reset_transition(
    node: &NodeId,
    from: &NodeState,
    reason: NodeResetReason,
    target: &NodeState,
) -> WorkflowResult<()> {
    let allowed = matches!(
        (from, reason, target),
        (
            NodeState::Running { .. },
            NodeResetReason::InterruptedRecovery,
            NodeState::Ready
        ) | (
            NodeState::Terminal {
                outcome: NodeTerminalState::Cancellation
            },
            NodeResetReason::CleanCancellation,
            NodeState::Pending | NodeState::Ready
        )
    );
    if allowed {
        Ok(())
    } else {
        Err(WorkflowError::IllegalTransition {
            node: node.clone(),
            from: format!("{from:?}"),
            to: format!("{target:?} ({reason:?})"),
        })
    }
}

pub(crate) fn derive_workflow_outcome(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
) -> Option<WorkflowOutcome> {
    if state.lifecycle != RunLifecycle::Completed {
        return None;
    }
    let required = workflow
        .graph
        .nodes
        .values()
        .filter(|node| !node.allow_failure);
    let mut outcomes = Vec::new();
    for node in required {
        let outcome = state.nodes.get(&node.id).and_then(NodeState::terminal)?;
        outcomes.push(outcome);
    }
    let mut cancellation = state.cancellation_requested
        && state.nodes.values().any(|node| {
            matches!(
                node,
                NodeState::Terminal {
                    outcome: NodeTerminalState::Cancellation
                }
            )
        });
    let mut denial = false;
    let mut failure = false;
    let mut blocked = false;
    for outcome in outcomes {
        match outcome {
            NodeTerminalState::Cancellation => cancellation = true,
            NodeTerminalState::Denial => denial = true,
            NodeTerminalState::Failure => failure = true,
            NodeTerminalState::Blocked => blocked = true,
            NodeTerminalState::Success | NodeTerminalState::Skipped => {}
        }
    }
    Some(if cancellation {
        WorkflowOutcome::Cancellation
    } else if denial {
        WorkflowOutcome::Denial
    } else if failure {
        WorkflowOutcome::Failure
    } else if blocked {
        WorkflowOutcome::Blocked
    } else {
        WorkflowOutcome::Success
    })
}

pub(crate) fn validate_lifecycle_transition(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
    target: RunLifecycle,
) -> WorkflowResult<Option<WorkflowOutcome>> {
    let allowed = state.lifecycle == target
        || matches!(
            (state.lifecycle, target),
            (RunLifecycle::Planned, RunLifecycle::Running)
                | (
                    RunLifecycle::Running,
                    RunLifecycle::Cancelling | RunLifecycle::NeedsRecovery
                )
                | (
                    RunLifecycle::Cancelling,
                    RunLifecycle::Completed | RunLifecycle::Running | RunLifecycle::NeedsRecovery
                )
                | (RunLifecycle::NeedsRecovery, RunLifecycle::Running)
                | (RunLifecycle::Completed, RunLifecycle::Running)
                | (RunLifecycle::Running, RunLifecycle::Completed)
        );
    if !allowed {
        return Err(WorkflowError::Scheduler(format!(
            "illegal workflow lifecycle transition from {:?} to {target:?}",
            state.lifecycle
        )));
    }
    if target != RunLifecycle::Completed {
        return Ok(None);
    }
    if state.nodes.values().any(|node| node.terminal().is_none()) {
        return Err(WorkflowError::Scheduler(
            "completed workflow contains non-terminal nodes".to_owned(),
        ));
    }
    let mut completed = state.clone();
    completed.lifecycle = RunLifecycle::Completed;
    derive_workflow_outcome(workflow, &completed)
        .map(Some)
        .ok_or_else(|| WorkflowError::Scheduler("completed workflow has no outcome".to_owned()))
}

#[cfg(test)]
#[path = "transition_tests.rs"]
mod tests;
