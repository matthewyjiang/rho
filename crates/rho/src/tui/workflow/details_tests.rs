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
        id: NodeId::new("review").unwrap(),
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
