use super::*;
use crate::workflow::{
    test_support::{agent_node, id, state, workflow},
    CommandNode, Node,
};

fn capacity() -> SchedulerCapacity {
    SchedulerCapacity {
        total: 8,
        agents: 8,
        commands: 8,
    }
}

fn command_node(name: &str, access: WorkspaceAccess) -> Node {
    let mut node = agent_node(name, &[], access);
    node.execution = NodeExecution::Command(CommandNode::Direct {
        executable: "command".to_owned(),
        arguments: Vec::new(),
        cwd: ".".to_owned(),
        output: None,
    });
    node
}

// Covers: map or completion order could change which ready nodes launch first.
// Owner: pure workflow scheduler.
#[test]
fn launches_ready_nodes_in_id_order_and_stable_prefix() {
    let workflow = workflow(vec![
        agent_node("z", &[], WorkspaceAccess::ReadOnly),
        agent_node("a", &[], WorkspaceAccess::ReadOnly),
        agent_node("m", &[], WorkspaceAccess::Mutating),
    ]);
    let mut state = state(&workflow);
    for node_state in state.nodes.values_mut() {
        *node_state = NodeState::Ready;
    }
    assert_eq!(
        next_actions(&workflow, &state, capacity()).unwrap(),
        vec![SchedulerAction::Launch {
            node: id("a"),
            access: WorkspaceAccess::ReadOnly,
        }]
    );
}

// Covers: a launch event must not bypass the durable ready transition.
// Owner: pure workflow scheduler.
#[test]
fn pending_nodes_transition_to_ready_before_launch() {
    let workflow = workflow(vec![agent_node("inspect", &[], WorkspaceAccess::Mutating)]);
    assert_eq!(
        next_actions(&workflow, &state(&workflow), capacity()).unwrap(),
        vec![SchedulerAction::MarkReady {
            node: id("inspect")
        }]
    );
}

// Covers: a full agent lane must not leave command capacity idle.
// Owner: pure workflow scheduler.
#[test]
fn full_kind_lane_does_not_block_other_kind() {
    let workflow = workflow(vec![
        agent_node("agent", &[], WorkspaceAccess::ReadOnly),
        command_node("command", WorkspaceAccess::ReadOnly),
    ]);
    let mut state = state(&workflow);
    for node_state in state.nodes.values_mut() {
        *node_state = NodeState::Ready;
    }
    let capacity = SchedulerCapacity {
        total: 2,
        agents: 0,
        commands: 1,
    };

    assert_eq!(
        next_actions(&workflow, &state, capacity).unwrap(),
        vec![SchedulerAction::Launch {
            node: id("command"),
            access: WorkspaceAccess::ReadOnly,
        }]
    );
}

// Covers: intended branch skips must not become errors, while failed ancestors must block.
// Owner: pure workflow scheduler.
#[test]
fn implicit_dependencies_preserve_skipped_and_blocked_rules() {
    for (dependency, expected) in [
        (NodeTerminalState::Skipped, NodeTerminalState::Skipped),
        (NodeTerminalState::Failure, NodeTerminalState::Blocked),
    ] {
        let workflow = workflow(vec![
            agent_node("first", &[], WorkspaceAccess::Mutating),
            agent_node("next", &["first"], WorkspaceAccess::Mutating),
        ]);
        let mut state = state(&workflow);
        state.nodes.insert(
            id("first"),
            NodeState::Terminal {
                outcome: dependency,
            },
        );
        assert_eq!(
            next_actions(&workflow, &state, capacity()).unwrap(),
            vec![SchedulerAction::MarkTerminal {
                node: id("next"),
                outcome: expected,
            }]
        );
    }
}
