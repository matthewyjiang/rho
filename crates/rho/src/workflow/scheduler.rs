use std::collections::BTreeMap;

use super::{
    evaluate_condition, validate_state_shape, ConditionContext, FrozenWorkflow, NodeExecution,
    NodeState, NodeTerminalState, RunLifecycle, SchedulerAction, SchedulerCapacity,
    ScopeInstanceId, ScopeState, TaskInstanceId, TruthValue, WorkflowResult, WorkflowState,
    WorkspaceAccess,
};

pub(crate) fn next_actions(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
    capacity: SchedulerCapacity,
) -> WorkflowResult<Vec<SchedulerAction>> {
    validate_state_shape(workflow, state)?;
    let local = state.root_scope();
    if !state.lifecycle.is_live() || local.result.is_some() {
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
    let statuses = local
        .nodes
        .iter()
        .filter_map(|(id, state)| state.terminal().map(|outcome| (id.clone(), outcome)))
        .collect::<BTreeMap<_, _>>();
    let context = ConditionContext {
        statuses: &statuses,
        command_exits: &local.command_exits,
        outputs: &local.outputs,
    };
    let mut actions = Vec::new();
    let mut runnable = Vec::new();
    let definition = workflow.program.scope(local.definition);
    for node in definition.nodes.values() {
        match local.nodes[&node.id] {
            NodeState::Ready => runnable.push(node),
            NodeState::Pending if dependencies_terminal(node, local) => {
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
    let running = local
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

fn dependencies_terminal(node: &super::Node, local: &ScopeState) -> bool {
    node.needs.iter().all(|id| {
        local
            .nodes
            .get(id)
            .is_some_and(|state| state.terminal().is_some())
    })
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
