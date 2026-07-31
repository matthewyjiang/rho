use super::{layer_index_of, render_dag};
use crate::{
    tui::workflow::event_adapter::{ExecutionMetadata, WorkflowNodeSnapshot},
    workflow::{AgentRuntime, NodeId, NodeState, WorkspaceAccess},
};
use pretty_assertions::assert_eq;

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

    let lines = render_dag(&nodes, 0, 80);
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
