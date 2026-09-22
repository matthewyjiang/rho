use pretty_assertions::assert_eq;

use super::*;
use crate::workflow::{
    test_support::{agent_node, id, state, workflow},
    NodeExecution, OutputPath, OutputReference, OutputSchema, WorkflowValue, WorkspaceAccess,
};

// Covers: resume cannot rerun a successful terminal task.
#[test]
fn terminal_nodes_cannot_return_to_ready() {
    assert!(matches!(
        validate_transition(
            &TaskInstanceId::root(id("node")),
            &NodeState::Terminal {
                outcome: NodeTerminalState::Success
            },
            &NodeState::Ready
        ),
        Err(WorkflowError::IllegalTransition { .. })
    ));
}

// Covers: scope aggregation respects allow_failure but never suppresses cancellation.
#[test]
fn local_required_outcomes_and_cancellation_determine_scope_result() {
    for (optional, child, expected) in [
        (true, NodeTerminalState::Failure, WorkflowOutcome::Success),
        (false, NodeTerminalState::Failure, WorkflowOutcome::Failure),
        (
            true,
            NodeTerminalState::Cancellation,
            WorkflowOutcome::Cancellation,
        ),
        (
            false,
            NodeTerminalState::Cancellation,
            WorkflowOutcome::Cancellation,
        ),
        (false, NodeTerminalState::Denial, WorkflowOutcome::Denial),
        (false, NodeTerminalState::Blocked, WorkflowOutcome::Blocked),
    ] {
        let mut node = agent_node("child", &[], WorkspaceAccess::ReadOnly);
        node.allow_failure = optional;
        let workflow = workflow(vec![node]);
        let mut state = state(&workflow);
        state
            .root_scope_mut()
            .nodes
            .insert(id("child"), NodeState::Terminal { outcome: child });
        let result = ScopeResult {
            outcome: expected,
            outputs: Default::default(),
        };
        assert_eq!(
            scope_result(&workflow, &state, ScopeInstanceId::ROOT).unwrap(),
            Some(result.clone())
        );
        assert_eq!(state.outcome(), None);
        assert!(validate_lifecycle_transition(&state, RunLifecycle::Completed).is_err());
        state.root_scope_mut().result = Some(result);
        state.lifecycle = RunLifecycle::Completed;
        assert_eq!(state.outcome(), Some(expected));
    }
}

// Covers: missing required exports block success while false is a real value.
#[test]
fn scope_exports_require_terminal_children_and_available_typed_values() {
    let mut node = agent_node("child", &[], WorkspaceAccess::ReadOnly);
    let NodeExecution::Agent(agent) = &mut node.execution else {
        unreachable!()
    };
    agent.output = Some(OutputSchema::Bool);
    let mut workflow = workflow(vec![node]);
    workflow.program.root.exports.insert(
        "answer".to_owned(),
        OutputReference {
            node: id("child"),
            path: OutputPath(vec![]),
        },
    );
    let mut state = state(&workflow);
    assert_eq!(
        scope_result(&workflow, &state, ScopeInstanceId::ROOT).unwrap(),
        None
    );
    state.root_scope_mut().nodes.insert(
        id("child"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        },
    );
    assert_eq!(
        scope_result(&workflow, &state, ScopeInstanceId::ROOT).unwrap(),
        Some(ScopeResult {
            outcome: WorkflowOutcome::Blocked,
            outputs: Default::default()
        })
    );
    state
        .root_scope_mut()
        .outputs
        .insert(id("child"), WorkflowValue::Bool(false));
    assert_eq!(
        scope_result(&workflow, &state, ScopeInstanceId::ROOT).unwrap(),
        Some(ScopeResult {
            outcome: WorkflowOutcome::Success,
            outputs: std::collections::BTreeMap::from([(
                "answer".to_owned(),
                WorkflowValue::Bool(false)
            )])
        })
    );
}
