use super::{render_dag, state_label, state_style, workflow_graph};
use crate::workflow::AttemptNumber;
use crate::{
    tui::{
        terminal_graph::{NodeStyle, RankOrdering},
        workflow::event_adapter::{ExecutionMetadata, WorkflowNodeSnapshot},
    },
    workflow::{AgentRuntime, NodeId, NodeState, NodeTerminalState, WorkspaceAccess},
};
use pretty_assertions::assert_eq;
use ratatui::style::Modifier;

use super::super::super::theme::Theme;

fn node(id: &str, name: &str, deps: &[&str], state: NodeState) -> WorkflowNodeSnapshot {
    WorkflowNodeSnapshot {
        id: NodeId::new(id).unwrap(),
        display_name: name.into(),
        dependencies: deps.iter().map(|dep| NodeId::new(*dep).unwrap()).collect(),
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

// Covers: a running node labels its in-flight attempt so the details pane and
// DAG distinguish retries (e.g. a resumed run showing `running · try 2`).
// Owner: workflow run TUI rendering (pure label logic).
#[test]
fn running_node_label_reports_attempt() {
    let attempt = AttemptNumber::new(2).unwrap();
    assert_eq!(
        state_label(&NodeState::Running { attempt }),
        "running · try 2"
    );
    assert_eq!(state_label(&NodeState::Pending), "waiting");
}

// Covers: ready stays visually stronger than pending without ANSI white/gray chrome
// Owner: workflow run TUI rendering (theme routing)
#[test]
fn state_styles_route_through_theme_and_keep_ready_distinct() {
    assert_eq!(state_style(&NodeState::Pending), Theme::dim());
    assert_eq!(state_style(&NodeState::Ready), Theme::text());
    assert_ne!(
        state_style(&NodeState::Pending),
        state_style(&NodeState::Ready)
    );
    assert_eq!(
        state_style(&NodeState::Running {
            attempt: AttemptNumber::new(1).unwrap()
        }),
        Theme::accent()
    );
    assert_eq!(
        state_style(&NodeState::Terminal {
            outcome: NodeTerminalState::Success
        }),
        Theme::success()
    );

    let running = NodeState::Running {
        attempt: AttemptNumber::new(1).unwrap(),
    };
    let nodes = vec![
        node("pending", "Pending", &[], NodeState::Pending),
        node("running", "Running", &[], running),
    ];
    let graph = workflow_graph(&nodes, 1, &[None, None]);
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| node.style)
            .collect::<Vec<_>>(),
        vec![
            NodeStyle::uniform(Theme::dim()),
            NodeStyle::new(
                Theme::accent().add_modifier(Modifier::BOLD),
                Theme::accent().add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ),
        ]
    );
}
