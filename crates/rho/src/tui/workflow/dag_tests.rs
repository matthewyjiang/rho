use super::{layer_index_of, render_dag, state_label, state_style};
use crate::workflow::AttemptNumber;
use crate::{
    tui::workflow::event_adapter::{ExecutionMetadata, WorkflowNodeSnapshot},
    workflow::{AgentRuntime, NodeId, NodeState, NodeTerminalState, WorkspaceAccess},
};
use pretty_assertions::assert_eq;

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

#[test]
fn dependents_render_below_dependencies() {
    let nodes = vec![
        node("apply", "Apply", &["inspect", "test"], NodeState::Pending),
        node("inspect", "Inspect", &[], NodeState::Pending),
        node("test", "Test", &[], NodeState::Pending),
    ];
    // indices: 0 apply, 1 inspect, 2 test
    assert_eq!(layer_index_of(&nodes, 1), Some(0));
    assert_eq!(layer_index_of(&nodes, 2), Some(0));
    assert_eq!(layer_index_of(&nodes, 0), Some(1));

    let lines = render_dag(&nodes, 0, 80, &vec![None; nodes.len()]);
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let inspect_pos = text.find("Inspect").expect("inspect label");
    let apply_pos = text.find("Apply").expect("apply label");
    assert!(inspect_pos < apply_pos);
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
}
