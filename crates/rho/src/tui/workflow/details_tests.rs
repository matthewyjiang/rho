use std::path::Path;

use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::DetailPane;
use crate::{
    tui::workflow::event_adapter::{ArtifactReference, ExecutionMetadata, WorkflowNodeSnapshot},
    workflow::{
        write_file_beneath, AgentRuntime, ArtifactKind, ArtifactObservation, ArtifactRef, Digest,
        NodeId, NodeState, NodeTerminalState, WorkspaceAccess,
    },
};

fn finished_node(relative: &str, bytes: &[u8]) -> WorkflowNodeSnapshot {
    WorkflowNodeSnapshot {
        id: crate::workflow::TaskInstanceId::root(NodeId::new("review").unwrap()),
        display_name: "Review".into(),
        dependencies: Vec::new(),
        access: WorkspaceAccess::ReadOnly,
        execution: ExecutionMetadata::Agent {
            name: "reviewer".into(),
            runtime: AgentRuntime::Rho,
            provider: None,
            model: None,
        },
        work: "review".into(),
        state: NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        },
        current_attempt: None,
        command_exit: None,
        validated_output: None,
        artifacts: vec![ArtifactReference {
            kind: ArtifactKind::AgentAnswer,
            artifact: ArtifactRef {
                relative_path: relative.into(),
                retained_bytes: bytes.len() as u64,
                observed: ArtifactObservation::Complete {
                    observed_bytes: bytes.len() as u64,
                },
                digest: Digest("sha256:dd".into()),
            },
        }],
        terminal_reason: None,
    }
}

// Covers: finished output opens top-anchored and stays there while scrolling.
// Owner: workflow details pane.
#[test]
fn finished_output_loads_and_scrolls_from_top() {
    let dir = tempdir().unwrap();
    let relative = "artifacts/review/answer.txt";
    let body = "# Review\n\n".to_owned() + &"line\n".repeat(40);
    write_file_beneath(dir.path(), Path::new(relative), body.as_bytes()).unwrap();

    let node = finished_node(relative, body.as_bytes());
    let mut pane = DetailPane::default();
    pane.set_run_directory(Some(dir.path().to_path_buf()));
    pane.refresh(Some(&node), /*reset_scroll*/ true);
    assert!(pane.body().is_some());

    let lines = pane.prepare_body_lines(40);
    pane.sync_geometry(ratatui::layout::Rect::new(0, 0, 40, 5), lines, 5);
    assert_eq!(pane.visible_start(), 0);
    assert!(pane.is_scrollable());
    pane.scroll_by(3);
    assert_eq!(pane.visible_start(), 3);
    assert_eq!(pane.visible_body_lines().len(), 5);
}

// Covers: a drag over the body selects in content-line space (so the copy
// matches the scrolled view), a release queues the selected text once, and a
// press outside the pane drops the highlight.
// Owner: workflow details pane.
#[test]
fn drag_over_body_selects_and_queues_copy() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let dir = tempdir().unwrap();
    let relative = "artifacts/review/answer.txt";
    // Blank-line paragraphs so markdown keeps one row per paragraph.
    let body = (0..20)
        .map(|n| format!("row {n:02}\n\n"))
        .collect::<String>();
    write_file_beneath(dir.path(), Path::new(relative), body.as_bytes()).unwrap();
    let node = finished_node(relative, body.as_bytes());
    let mut pane = DetailPane::default();
    pane.set_run_directory(Some(dir.path().to_path_buf()));
    pane.refresh(Some(&node), /*reset_scroll*/ true);
    let area = ratatui::layout::Rect::new(10, 5, 21, 4);
    let lines = pane.prepare_body_lines(20);
    pane.sync_geometry(area, lines, 4);
    pane.scroll_by(5);
    let top = pane.visible_start();

    assert!(pane.handle_mouse(MouseEventKind::Down(MouseButton::Left), 10, 5));
    assert!(pane.handle_mouse(MouseEventKind::Drag(MouseButton::Left), 15, 6));
    assert_eq!(pane.take_pending_copy(), None, "drag alone must not copy");
    pane.handle_mouse(MouseEventKind::Up(MouseButton::Left), 15, 6);
    let visible = pane
        .visible_body_lines()
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    // Columns 0..=5 of the second visible row: the press was at x=10, the
    // release at x=15.
    let second = visible[1].chars().take(6).collect::<String>();
    assert_eq!(
        pane.take_pending_copy(),
        Some(format!("{}\n{}", visible[0].trim_end(), second.trim_end())),
        "top line {top}"
    );
    assert_eq!(pane.take_pending_copy(), None);
    assert!(pane.selection().is_some());

    assert!(pane.handle_mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
    assert_eq!(pane.selection(), None);
}
