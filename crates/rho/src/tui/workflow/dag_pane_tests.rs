use super::{centered_canvas, DagMouse, DagPane};
use crate::{
    tui::workflow::{
        dag::render_dag,
        event_adapter::{ExecutionMetadata, WorkflowNodeSnapshot},
    },
    workflow::{AgentRuntime, NodeId, NodeState, WorkspaceAccess},
};
use crossterm::event::{MouseButton, MouseEventKind};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

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

// Covers: a click must select the node under the pointer through the pane's
// scroll offset, and a drag must pan the canvas instead of selecting.
// Owner: workflow DAG pane mouse mapping (pure geometry).
#[test]
fn mouse_click_selects_and_drag_pans() {
    let nodes = vec![
        node("inspect", "Inspect workspace", &[], NodeState::Pending),
        node("test", "Run checks", &[], NodeState::Pending),
        node("apply", "Apply", &["inspect", "test"], NodeState::Pending),
    ];
    let rendered = render_dag(&nodes, 0, &vec![None; nodes.len()]);
    let mut pane = DagPane::default();
    let inner = Rect::new(2, 1, 30, 8);
    let offset = pane.offset_for_draw(&rendered, 0, inner);
    assert_eq!(offset, (0, 0));

    // Click the center of the bottom node, mapped through the pane origin.
    let target = rendered.node_rects[2];
    let column = inner.x + (target.x + target.width / 2) as u16;
    let row = inner.y + (target.y + target.height / 2) as u16;
    assert_eq!(
        pane.handle_mouse(MouseEventKind::Down(MouseButton::Left), column, row),
        DagMouse::Redraw
    );
    assert_eq!(
        pane.handle_mouse(MouseEventKind::Up(MouseButton::Left), column, row),
        DagMouse::SelectNode(2)
    );

    // Dragging left pulls the canvas left: the offset grows and release is
    // not a click.
    assert_eq!(
        pane.handle_mouse(MouseEventKind::Down(MouseButton::Left), column, row),
        DagMouse::Redraw
    );
    assert_eq!(
        pane.handle_mouse(MouseEventKind::Drag(MouseButton::Left), column - 4, row),
        DagMouse::Redraw
    );
    assert_eq!(
        pane.handle_mouse(MouseEventKind::Up(MouseButton::Left), column - 4, row),
        DagMouse::Redraw
    );
    assert_eq!(pane.offset_for_draw(&rendered, 0, inner), (0, 4));

    // Keyboard navigation resumes following the selection.
    pane.clear_manual_offset();
    assert_eq!(pane.offset_for_draw(&rendered, 0, inner), (0, 0));
}

// Covers: a graph smaller than the pane must render centered instead of
// pinned to the top-left, per axis, while an oversized axis keeps the full
// pane extent so scrolling still works.
// Owner: workflow DAG pane draw geometry (pure geometry).
#[test]
fn small_canvas_centers_in_the_pane() {
    let inner = Rect::new(2, 1, 30, 8);
    let cases = [
        // Fits on both axes: centered on both.
        ((10, 4), Rect::new(12, 3, 10, 4)),
        // Wider than the pane: full width, centered vertically.
        ((100, 4), Rect::new(2, 3, 30, 4)),
        // Taller than the pane: full height, centered horizontally.
        ((10, 40), Rect::new(12, 1, 10, 8)),
        // Oversized on both axes: the pane rect is unchanged.
        ((100, 40), inner),
    ];
    for (canvas, expected) in cases {
        assert_eq!(
            centered_canvas(inner, canvas),
            expected,
            "canvas {canvas:?}"
        );
    }
}

// Covers: clicks must map to the node under the pointer when the pane was
// drawn into a centered rect rather than the full pane.
// Owner: workflow DAG pane mouse mapping (pure geometry).
#[test]
fn click_maps_through_a_centered_draw_rect() {
    let nodes = vec![
        node("inspect", "Inspect workspace", &[], NodeState::Pending),
        node("test", "Run checks", &[], NodeState::Pending),
    ];
    let rendered = render_dag(&nodes, 0, &vec![None; nodes.len()]);
    let mut pane = DagPane::default();
    let pane_rect = Rect::new(0, 0, 80, 24);
    let inner = centered_canvas(pane_rect, (rendered.canvas_width, rendered.canvas_height));
    assert!(inner.x > pane_rect.x && inner.y > pane_rect.y);
    pane.offset_for_draw(&rendered, 0, inner);

    let target = rendered.node_rects[1];
    let column = inner.x + (target.x + target.width / 2) as u16;
    let row = inner.y + (target.y + target.height / 2) as u16;
    pane.handle_mouse(MouseEventKind::Down(MouseButton::Left), column, row);
    assert_eq!(
        pane.handle_mouse(MouseEventKind::Up(MouseButton::Left), column, row),
        DagMouse::SelectNode(1)
    );
    // A click in the pane margin outside the centered canvas is not a hit.
    assert_eq!(
        pane.handle_mouse(MouseEventKind::Down(MouseButton::Left), 0, 0),
        DagMouse::Ignored
    );
}
