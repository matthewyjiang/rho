use super::*;
use crate::workflow::{
    test_support::{agent_node, id, state, workflow},
    WorkspaceAccess,
};

// Covers: resume could rerun terminal nodes through an illegal state transition.
// Owner: workflow transition core.
#[test]
fn terminal_nodes_cannot_return_to_ready() {
    assert!(matches!(
        validate_transition(
            &id("node"),
            &NodeState::Terminal {
                outcome: NodeTerminalState::Success
            },
            &NodeState::Ready
        ),
        Err(WorkflowError::IllegalTransition { .. })
    ));
}

// Covers: allowed failures could incorrectly fail the whole workflow.
// Owner: workflow outcome core.
#[test]
fn allowed_failure_does_not_change_workflow_outcome() {
    let mut optional = agent_node("optional", &[], WorkspaceAccess::Mutating);
    optional.allow_failure = true;
    let workflow = workflow(vec![
        agent_node("required", &[], WorkspaceAccess::Mutating),
        optional,
    ]);
    let mut state = state(&workflow);
    state.lifecycle = RunLifecycle::Completed;
    state.nodes.insert(
        id("required"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        },
    );
    state.nodes.insert(
        id("optional"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Failure,
        },
    );
    assert_eq!(
        derive_workflow_outcome(&workflow, &state),
        Some(WorkflowOutcome::Success)
    );
}

// Covers: a completed lifecycle could make pending work appear durably finished.
// Owner: workflow transition core.
#[test]
fn completed_lifecycle_requires_every_node_to_be_terminal() {
    let workflow = workflow(vec![agent_node("required", &[], WorkspaceAccess::Mutating)]);
    let mut state = state(&workflow);
    state.lifecycle = RunLifecycle::Running;

    assert!(matches!(
        validate_lifecycle_transition(&workflow, &state, RunLifecycle::Completed),
        Err(WorkflowError::Scheduler(message))
            if message == "completed workflow contains non-terminal nodes"
    ));
}

// Covers: executor cancellation of optional work must still cancel the workflow.
// Owner: workflow outcome core.
#[test]
fn optional_node_cancellation_derives_cancellation_without_request() {
    let mut optional = agent_node("optional", &[], WorkspaceAccess::Mutating);
    optional.allow_failure = true;
    let workflow = workflow(vec![
        agent_node("required", &[], WorkspaceAccess::Mutating),
        optional,
    ]);
    let mut state = state(&workflow);
    state.lifecycle = RunLifecycle::Completed;
    state.nodes.insert(
        id("required"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        },
    );
    state.nodes.insert(
        id("optional"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Cancellation,
        },
    );

    assert_eq!(
        derive_workflow_outcome(&workflow, &state),
        Some(WorkflowOutcome::Cancellation)
    );
}

// Covers: an executor cancellation without a request must not look like workflow success.
// Owner: workflow outcome core.
#[test]
fn required_node_cancellation_derives_cancellation() {
    let workflow = workflow(vec![agent_node("required", &[], WorkspaceAccess::Mutating)]);
    let mut state = state(&workflow);
    state.lifecycle = RunLifecycle::Completed;
    state.nodes.insert(
        id("required"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Cancellation,
        },
    );

    assert_eq!(
        derive_workflow_outcome(&workflow, &state),
        Some(WorkflowOutcome::Cancellation)
    );
}
