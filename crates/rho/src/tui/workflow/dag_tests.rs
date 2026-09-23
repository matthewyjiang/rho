use super::{render_dag, workflow_graph};
use crate::{
    tui::{
        terminal_graph::RankOrdering,
        workflow::event_adapter::{ExecutionMetadata, WorkflowNodeSnapshot},
    },
    workflow::{AgentRuntime, NodeId, NodeState, WorkspaceAccess},
};
use pretty_assertions::assert_eq;

fn node(id: &str, name: &str, deps: &[&str], state: NodeState) -> WorkflowNodeSnapshot {
    WorkflowNodeSnapshot {
        id: crate::workflow::TaskInstanceId::root(NodeId::new(id).unwrap()),
        display_name: name.into(),
        dependencies: deps
            .iter()
            .map(|dep| crate::workflow::TaskInstanceId::root(NodeId::new(*dep).unwrap()))
            .collect(),
        access: WorkspaceAccess::ReadOnly,
        execution: ExecutionMetadata::Agent {
            name: "agent".into(),
            runtime: AgentRuntime::Rho,
            provider: None,
            model: None,
        },
        work: format!("work for {name}"),
        state,
        current_attempt: None,
        command_exit: None,
        validated_output: None,
        artifacts: Vec::new(),
        terminal_reason: None,
    }
}

// Covers: workflow dependencies must remain distinct routed graph edges.
// Owner: workflow-to-terminal-graph adapter.
#[test]
fn dependencies_render_as_edges_below_their_parents() {
    let nodes = vec![
        node("apply", "Apply", &["inspect", "test"], NodeState::Pending),
        node("inspect", "Inspect", &[], NodeState::Pending),
        node("test", "Test", &[], NodeState::Pending),
    ];
    let activities = vec![None; nodes.len()];
    let graph = workflow_graph(&nodes, 0, &activities);
    assert_eq!(
        graph
            .edges
            .iter()
            .map(|edge| (edge.from, edge.to))
            .collect::<Vec<_>>(),
        vec![(1, 0), (2, 0)]
    );
    assert_eq!(graph.rank_ordering, RankOrdering::PreserveInput);

    let rendered = render_dag(&nodes, 0, &activities);
    assert!(rendered.node_rects[1].y < rendered.node_rects[0].y);
    assert!(rendered.node_rects[2].y < rendered.node_rects[0].y);
}

// Covers: graph traversal must keep the selected node inside a clipped pane.
// Owner: workflow graph viewport math.
#[test]
fn viewport_follows_the_selected_node() {
    let nodes = vec![
        node("inspect", "Inspect", &[], NodeState::Pending),
        node("test", "Test", &["inspect"], NodeState::Pending),
        node("apply", "Apply", &["test"], NodeState::Pending),
    ];
    let rendered = render_dag(&nodes, 2, &vec![None; nodes.len()]);
    let (row, column) = rendered.viewport_offset(2, 5, 5);
    let selected = rendered.node_rects[2];

    assert!(selected.y >= usize::from(row));
    assert!(selected.y < usize::from(row) + 5);
    assert_eq!(column, selected.x as u16);
    assert!(selected.x >= usize::from(column));
    assert!(selected.x < usize::from(column) + 5);
}

// Covers: arbitrary progress messages must not consume the graph's render budget.
// Owner: workflow-to-terminal-graph adapter.
#[test]
fn progress_activity_keeps_the_graph_compact() {
    let nodes = vec![node("inspect", "Inspect", &[], NodeState::Pending)];
    // Keep the fixture far beyond the 28-column activity contract.
    let activities = vec![Some("still checking ".repeat(100))];
    let graph = workflow_graph(&nodes, 0, &activities);
    let activity = graph.nodes[0]
        .label
        .rsplit_once(" · ")
        .expect("activity is present")
        .1;

    assert_eq!(unicode_width::UnicodeWidthStr::width(activity), 28);
    assert!(activity.ends_with('…'));
    assert_eq!(render_dag(&nodes, 0, &activities).node_rects.len(), 1);
}

// Covers: hjkl must move to the spatially nearest node on the rendered
// canvas, so `j` from any node in a row reaches the row below even when the
// graph order lists other siblings first.
// Owner: workflow DAG spatial navigation (pure geometry policy).
#[test]
fn hjkl_selects_the_spatially_nearest_node() {
    use super::{spatial_neighbor, SpatialDirection};
    let nodes = vec![
        node("inspect", "Inspect workspace", &[], NodeState::Pending),
        node("test", "Run checks", &[], NodeState::Pending),
        node("apply", "Apply", &["inspect", "test"], NodeState::Pending),
    ];
    let rendered = render_dag(&nodes, 0, &vec![None; nodes.len()]);
    let rects = &rendered.node_rects;

    let cases = [
        (1, SpatialDirection::Down, Some(2)),
        (0, SpatialDirection::Down, Some(2)),
        (0, SpatialDirection::Right, Some(1)),
        (1, SpatialDirection::Left, Some(0)),
        (2, SpatialDirection::Down, None),
        (0, SpatialDirection::Up, None),
    ];
    for (selected, direction, expected) in cases {
        assert_eq!(
            spatial_neighbor(rects, selected, direction),
            expected,
            "from {selected} going {direction:?}"
        );
    }
    // The bottom node sits between both parents; `k` resolves the tie to the
    // earlier node so keyboard round trips stay deterministic.
    assert_eq!(spatial_neighbor(rects, 2, SpatialDirection::Up), Some(0));
}
