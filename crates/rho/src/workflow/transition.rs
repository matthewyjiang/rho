use super::{
    FrozenWorkflow, NodeId, NodeState, NodeTerminalState, RunLifecycle, WorkflowError,
    WorkflowOutcome, WorkflowResult, WorkflowState,
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
                    outcome: NodeTerminalState::Skipped | NodeTerminalState::Blocked
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
    let mut cancellation = false;
    let mut denial = false;
    let mut failure = false;
    let mut blocked = false;
    for outcome in outcomes {
        match outcome {
            NodeTerminalState::Cancellation => cancellation = state.cancellation_requested,
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

#[cfg(test)]
#[path = "transition_tests.rs"]
mod tests;
