use std::collections::BTreeMap;

use super::{
    evaluate_condition, validate_lifecycle_transition, validate_reset_transition,
    validate_transition, ConditionContext, FrozenWorkflow, NodeExecution, NodeState,
    NodeTerminalState, RunLifecycle, SchedulerAction, SchedulerCapacity, SchedulerEvent,
    ScopeInstanceId, TaskInstanceId, TruthValue, WorkflowError, WorkflowResult, WorkflowState,
    WorkspaceAccess,
};

pub(crate) fn next_actions(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
    capacity: SchedulerCapacity,
) -> WorkflowResult<Vec<SchedulerAction>> {
    validate_state_shape(workflow, state)?;
    if !state.lifecycle.is_live() || state.root_scope().result.is_some() {
        return Ok(Vec::new());
    }
    if let Some(result) = super::scope_result(workflow, state, ScopeInstanceId::ROOT)? {
        return Ok(vec![SchedulerAction::FinishScope {
            scope: ScopeInstanceId::ROOT,
            result,
        }]);
    }
    if state.cancellation_requested || state.lifecycle == RunLifecycle::Cancelling {
        return Ok(Vec::new());
    }
    let statuses = state
        .root_scope()
        .nodes
        .iter()
        .filter_map(|(id, state)| state.terminal().map(|outcome| (id.clone(), outcome)))
        .collect::<BTreeMap<_, _>>();
    let context = ConditionContext {
        statuses: &statuses,
        command_exits: &state.root_scope().command_exits,
        outputs: &state.root_scope().outputs,
    };
    let mut actions = Vec::new();
    let mut runnable = Vec::new();
    let definition = &workflow.program.root;
    for node in definition.nodes.values() {
        match state.root_scope().nodes[&node.id] {
            NodeState::Ready => runnable.push(node),
            NodeState::Pending if dependencies_terminal(node, state) => {
                match node_decision(node, &context) {
                    NodeDecision::Run => actions.push(SchedulerAction::MarkReady {
                        node: TaskInstanceId::root(node.id.clone()),
                    }),
                    NodeDecision::Terminal(outcome) => {
                        actions.push(SchedulerAction::MarkTerminal {
                            node: TaskInstanceId::root(node.id.clone()),
                            outcome,
                        })
                    }
                }
            }
            NodeState::Pending | NodeState::Running { .. } | NodeState::Terminal { .. } => {}
        }
    }

    let running = state
        .root_scope()
        .nodes
        .iter()
        .filter(|(_, node_state)| matches!(node_state, NodeState::Running { .. }))
        .map(|(id, _)| &definition.nodes[id]);
    let mut use_total = 0_u32;
    let mut use_agents = 0_u32;
    let mut use_commands = 0_u32;
    let mut readers = 0_u32;
    let mut writer = false;
    for node in running {
        use_total += 1;
        match node.execution {
            NodeExecution::Agent(_) => use_agents += 1,
            NodeExecution::Command(_) => use_commands += 1,
        }
        match node.access {
            WorkspaceAccess::ReadOnly => readers += 1,
            WorkspaceAccess::Mutating => writer = true,
        }
    }
    let total_limit = capacity.total.min(workflow.scheduler.max_parallel_nodes);
    let agent_limit = capacity.agents.min(workflow.scheduler.max_parallel_agents);
    let command_limit = capacity
        .commands
        .min(workflow.scheduler.max_parallel_commands);
    for node in runnable {
        if use_total >= total_limit {
            break;
        }
        let kind_fits = match node.execution {
            NodeExecution::Agent(_) => use_agents < agent_limit,
            NodeExecution::Command(_) => use_commands < command_limit,
        };
        if !kind_fits {
            continue;
        }
        let access_fits = match node.access {
            WorkspaceAccess::ReadOnly => !writer,
            WorkspaceAccess::Mutating => !writer && readers == 0,
        };
        if !access_fits {
            break;
        }
        actions.push(SchedulerAction::Launch {
            node: TaskInstanceId::root(node.id.clone()),
            access: node.access,
        });
        use_total += 1;
        match node.execution {
            NodeExecution::Agent(_) => use_agents += 1,
            NodeExecution::Command(_) => use_commands += 1,
        }
        match node.access {
            WorkspaceAccess::ReadOnly => readers += 1,
            WorkspaceAccess::Mutating => writer = true,
        }
    }
    Ok(actions)
}

enum NodeDecision {
    Run,
    Terminal(NodeTerminalState),
}

fn node_decision(node: &super::Node, context: &ConditionContext<'_>) -> NodeDecision {
    if let Some(condition) = &node.condition {
        return match evaluate_condition(condition, context) {
            TruthValue::True => NodeDecision::Run,
            TruthValue::False => NodeDecision::Terminal(NodeTerminalState::Skipped),
            TruthValue::Unavailable => NodeDecision::Terminal(NodeTerminalState::Blocked),
        };
    }
    let outcomes = node.needs.iter().filter_map(|id| context.statuses.get(id));
    let mut skipped = false;
    for outcome in outcomes {
        match outcome {
            NodeTerminalState::Success => {}
            NodeTerminalState::Skipped => skipped = true,
            NodeTerminalState::Failure
            | NodeTerminalState::Denial
            | NodeTerminalState::Cancellation
            | NodeTerminalState::Blocked => {
                return NodeDecision::Terminal(NodeTerminalState::Blocked)
            }
        }
    }
    if skipped {
        NodeDecision::Terminal(NodeTerminalState::Skipped)
    } else {
        NodeDecision::Run
    }
}

fn dependencies_terminal(node: &super::Node, state: &WorkflowState) -> bool {
    node.needs.iter().all(|id| {
        state
            .root_scope()
            .nodes
            .get(id)
            .is_some_and(|state| state.terminal().is_some())
    })
}

pub(crate) fn apply_event(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
    event: SchedulerEvent,
) -> WorkflowResult<WorkflowState> {
    validate_state_shape(workflow, state)?;
    let mut next = state.clone();
    match event {
        SchedulerEvent::MarkReady { node } => replace_node(&mut next, node, NodeState::Ready)?,
        SchedulerEvent::Launched { node, attempt } => {
            replace_node(&mut next, node, NodeState::Running { attempt })?
        }
        SchedulerEvent::Finished { node, completion } => {
            ensure_open_task(&next, &node)?;
            let definition = workflow
                .program
                .root
                .nodes
                .get(node.definition())
                .ok_or_else(|| {
                    WorkflowError::Scheduler(format!("event targets unknown node '{node}'"))
                })?;
            if let Some(output) = &completion.structured_output {
                let value = &output.value;
                let output_bytes = serde_json::to_vec(value)?.len() as u64;
                if output_bytes > definition.max_output_bytes {
                    return Err(WorkflowError::Scheduler(format!(
                        "node '{node}' output budget is {} bytes but observed {output_bytes} bytes",
                        definition.max_output_bytes
                    )));
                }
                definition
                    .execution
                    .output_schema()
                    .ok_or_else(|| {
                        WorkflowError::Scheduler(format!(
                            "node '{node}' produced structured output without a schema"
                        ))
                    })?
                    .validate_value(value)?;
                next.root_scope_mut()
                    .outputs
                    .insert(node.definition().clone(), value.clone());
            }
            if let Some(exit) = &completion.command_exit {
                if !matches!(definition.execution, NodeExecution::Command(_)) {
                    return Err(WorkflowError::Scheduler(format!(
                        "agent node '{node}' reported a command exit"
                    )));
                }
                next.root_scope_mut()
                    .command_exits
                    .insert(node.definition().clone(), exit.clone());
            }
            replace_node(
                &mut next,
                node.clone(),
                NodeState::Terminal {
                    outcome: completion.outcome,
                },
            )?;
            next.root_scope_mut()
                .completions
                .insert(node.definition().clone(), *completion);
        }
        SchedulerEvent::CancellationRequested => {
            validate_lifecycle_transition(state, RunLifecycle::Cancelling)?;
            next.cancellation_requested = true;
            next.lifecycle = RunLifecycle::Cancelling;
        }
        SchedulerEvent::ResetNode { node, reason } => {
            ensure_open_task(&next, &node)?;
            let current = next.task(&node).ok_or_else(|| {
                WorkflowError::Scheduler(format!("event targets unknown node '{node}'"))
            })?;
            let target = match reason {
                super::NodeResetReason::InterruptedRecovery => NodeState::Ready,
                super::NodeResetReason::CleanCancellation => match next
                    .completion(&node)
                    .and_then(|completion| completion.cancellation_resume)
                {
                    Some(super::CancellationResumeState::Pending) => NodeState::Pending,
                    Some(super::CancellationResumeState::Ready) => NodeState::Ready,
                    None => {
                        return Err(WorkflowError::Scheduler(format!(
                            "cancelled node '{node}' has no resume state"
                        )))
                    }
                },
            };
            validate_reset_transition(&node, current, reason, &target)?;
            let local = next.root_scope_mut();
            local.nodes.insert(node.definition().clone(), target);
            local.command_exits.remove(node.definition());
            local.outputs.remove(node.definition());
            local.completions.remove(node.definition());
        }
    }
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or_else(|| WorkflowError::Scheduler("state revision overflow".to_owned()))?;
    Ok(next)
}

fn replace_node(
    state: &mut WorkflowState,
    node: TaskInstanceId,
    target: NodeState,
) -> WorkflowResult<()> {
    ensure_open_task(state, &node)?;
    let current = state
        .task(&node)
        .ok_or_else(|| WorkflowError::Scheduler(format!("event targets unknown node '{node}'")))?;
    validate_transition(&node, current, &target)?;
    state
        .root_scope_mut()
        .nodes
        .insert(node.definition().clone(), target);
    Ok(())
}

pub(crate) fn validate_state_shape(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
) -> WorkflowResult<()> {
    if state.scopes.len() != 1 || !state.scopes.contains_key(&ScopeInstanceId::ROOT) {
        return Err(WorkflowError::Scheduler(
            "program requires exactly the root scope instance".to_owned(),
        ));
    }
    let local = state.root_scope();
    let definition = &workflow.program.root;
    local.durable.validate_membership(&local.nodes)?;
    if definition.nodes.len() != local.nodes.len() || definition.nodes.keys().ne(local.nodes.keys())
    {
        return Err(WorkflowError::Scheduler(
            "node state keys differ from root scope node keys".to_owned(),
        ));
    }
    if local
        .outputs
        .keys()
        .chain(local.command_exits.keys())
        .chain(local.completions.keys())
        .any(|id| !local.nodes.contains_key(id))
    {
        return Err(WorkflowError::Scheduler(
            "scope data references an unknown local task".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_open_task(state: &WorkflowState, task: &TaskInstanceId) -> WorkflowResult<()> {
    if task.scope() != ScopeInstanceId::ROOT
        || state.task(task).is_none()
        || state.root_scope().result.is_some()
    {
        return Err(WorkflowError::Scheduler(format!(
            "task '{task}' does not belong to an open scope"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
