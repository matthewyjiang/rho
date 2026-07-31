use std::collections::BTreeMap;

use super::{
    evaluate_condition, validate_transition, ConditionContext, FrozenWorkflow, NodeExecution,
    NodeId, NodeState, NodeTerminalState, RunLifecycle, SchedulerAction, SchedulerCapacity,
    SchedulerEvent, TruthValue, WorkflowError, WorkflowResult, WorkflowState, WorkspaceAccess,
};

pub(crate) fn next_actions(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
    capacity: SchedulerCapacity,
) -> WorkflowResult<Vec<SchedulerAction>> {
    validate_state_shape(workflow, state)?;
    if state.cancellation_requested || state.lifecycle == RunLifecycle::NeedsRecovery {
        return Ok(Vec::new());
    }
    let statuses = state
        .nodes
        .iter()
        .filter_map(|(id, state)| state.terminal().map(|outcome| (id.clone(), outcome)))
        .collect::<BTreeMap<_, _>>();
    let context = ConditionContext {
        statuses: &statuses,
        command_exits: &state.command_exits,
        outputs: &state.outputs,
    };
    let mut actions = Vec::new();
    let mut runnable = Vec::new();
    for node in workflow.graph.nodes.values() {
        match state.nodes[&node.id] {
            NodeState::Ready => runnable.push(node),
            NodeState::Pending if dependencies_terminal(node, state) => {
                match node_decision(node, &context) {
                    NodeDecision::Run => actions.push(SchedulerAction::MarkReady {
                        node: node.id.clone(),
                    }),
                    NodeDecision::Terminal(outcome) => {
                        actions.push(SchedulerAction::MarkTerminal {
                            node: node.id.clone(),
                            outcome,
                        })
                    }
                }
            }
            NodeState::Pending | NodeState::Running { .. } | NodeState::Terminal { .. } => {}
        }
    }

    let running = state
        .nodes
        .iter()
        .filter(|(_, node_state)| matches!(node_state, NodeState::Running { .. }))
        .map(|(id, _)| &workflow.graph.nodes[id]);
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
        let kind_fits = match node.execution {
            NodeExecution::Agent(_) => use_agents < agent_limit,
            NodeExecution::Command(_) => use_commands < command_limit,
        };
        let access_fits = match node.access {
            WorkspaceAccess::ReadOnly => !writer,
            WorkspaceAccess::Mutating => !writer && readers == 0,
        };
        if use_total >= total_limit || !kind_fits || !access_fits {
            break;
        }
        actions.push(SchedulerAction::Launch {
            node: node.id.clone(),
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
        SchedulerEvent::Finished {
            node,
            outcome,
            command_exit,
            output,
        } => {
            let definition = workflow.graph.nodes.get(&node).ok_or_else(|| {
                WorkflowError::Scheduler(format!("event targets unknown node '{node}'"))
            })?;
            if let Some(value) = &output {
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
                next.outputs.insert(node.clone(), value.clone());
            }
            if let Some(exit) = command_exit {
                if !matches!(definition.execution, NodeExecution::Command(_)) {
                    return Err(WorkflowError::Scheduler(format!(
                        "agent node '{node}' reported a command exit"
                    )));
                }
                next.command_exits.insert(node.clone(), exit);
            }
            replace_node(&mut next, node, NodeState::Terminal { outcome })?;
        }
        SchedulerEvent::CancellationRequested => {
            next.cancellation_requested = true;
            next.lifecycle = RunLifecycle::Cancelling;
        }
    }
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or_else(|| WorkflowError::Scheduler("state revision overflow".to_owned()))?;
    Ok(next)
}

fn replace_node(state: &mut WorkflowState, node: NodeId, target: NodeState) -> WorkflowResult<()> {
    let current = state
        .nodes
        .get(&node)
        .ok_or_else(|| WorkflowError::Scheduler(format!("event targets unknown node '{node}'")))?;
    validate_transition(&node, current, &target)?;
    state.nodes.insert(node, target);
    Ok(())
}

fn validate_state_shape(workflow: &FrozenWorkflow, state: &WorkflowState) -> WorkflowResult<()> {
    if workflow.graph.nodes.len() != state.nodes.len()
        || workflow.graph.nodes.keys().ne(state.nodes.keys())
    {
        return Err(WorkflowError::Scheduler(
            "node state keys differ from frozen graph keys".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
