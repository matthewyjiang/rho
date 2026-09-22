use std::collections::BTreeMap;

use super::{
    FrozenWorkflow, NodeResetReason, NodeState, NodeTerminalState, RunLifecycle, ScopeInstanceId,
    ScopeResult, TaskInstanceId, WorkflowError, WorkflowOutcome, WorkflowResult, WorkflowState,
};

pub(crate) fn validate_transition(
    node: &TaskInstanceId,
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
    node: &TaskInstanceId,
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

/// Compute closure only after every child has a terminal outcome.
/// Exports are required even from skipped or allow_failure children: an absent
/// exported value turns an otherwise successful scope into Blocked.
pub(crate) fn scope_result(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
    scope: ScopeInstanceId,
) -> WorkflowResult<Option<ScopeResult>> {
    let local = state
        .scope(scope)
        .ok_or_else(|| WorkflowError::Scheduler(format!("unknown scope '{scope}'")))?;
    if local.nodes.values().any(|node| node.terminal().is_none()) {
        return Ok(None);
    }
    let definition = workflow.program.scope(local.definition);
    let required = definition.nodes.values().filter(|node| !node.allow_failure);
    let mut outcomes = Vec::new();
    for node in required {
        let outcome = local
            .nodes
            .get(&node.id)
            .and_then(NodeState::terminal)
            .ok_or_else(|| {
                WorkflowError::Scheduler(format!(
                    "scope '{scope}' has no terminal task '{}'",
                    node.id
                ))
            })?;
        outcomes.push(outcome);
    }
    let mut cancellation = local.nodes.values().any(|node| {
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
    let mut outcome = if cancellation {
        WorkflowOutcome::Cancellation
    } else if denial {
        WorkflowOutcome::Denial
    } else if failure {
        WorkflowOutcome::Failure
    } else if blocked {
        WorkflowOutcome::Blocked
    } else {
        WorkflowOutcome::Success
    };
    // A failed or cancelled scope has no exported result. In particular, export
    // validation must not obstruct cancellation after all attempts are stopped.
    if outcome != WorkflowOutcome::Success {
        return Ok(Some(ScopeResult {
            outcome,
            outputs: BTreeMap::new(),
        }));
    }
    let outputs = definition.resolve_exports(&local.outputs)?;
    if outputs.is_none() && outcome == WorkflowOutcome::Success {
        outcome = WorkflowOutcome::Blocked;
    }
    Ok(Some(ScopeResult {
        outcome,
        outputs: outputs.unwrap_or_default(),
    }))
}

pub(crate) enum LifecycleTransition {
    Advance(RunLifecycle),
    ReopenCancelledScope,
}

pub(crate) fn validate_lifecycle_transition(
    state: &WorkflowState,
    transition: LifecycleTransition,
) -> WorkflowResult<()> {
    let (target, cancelled_reopen) = match transition {
        LifecycleTransition::Advance(target) => (target, false),
        LifecycleTransition::ReopenCancelledScope => {
            if !state
                .run_result()
                .is_some_and(|result| result.outcome == WorkflowOutcome::Cancellation)
            {
                return Err(WorkflowError::Scheduler(
                    "only a cancelled closed scope can reopen".to_owned(),
                ));
            }
            (
                RunLifecycle::Cancelling,
                state.lifecycle == RunLifecycle::Completed,
            )
        }
    };
    let allowed = cancelled_reopen
        || state.lifecycle == target
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
    if target == RunLifecycle::Running && state.run_result().is_some() {
        return Err(WorkflowError::Scheduler(
            "closed root scope must be explicitly reopened before running".to_owned(),
        ));
    }
    if target != RunLifecycle::Completed {
        return Ok(());
    }
    state
        .run_result()
        .map(|_| ())
        .ok_or_else(|| WorkflowError::Scheduler("completed workflow has no outcome".to_owned()))
}

#[cfg(test)]
#[path = "transition_tests.rs"]
mod tests;
