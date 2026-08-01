use std::path::Path;

use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::{
    body_matches_node, kind_label, load_finished_output, primary_artifact, render_body_lines,
};
use crate::{
    tui::workflow::event_adapter::{ArtifactReference, ExecutionMetadata, WorkflowNodeSnapshot},
    workflow::{
        write_file_beneath, AgentRuntime, ArtifactKind, ArtifactObservation, ArtifactRef, Digest,
        NodeId, NodeState, NodeTerminalState, WorkspaceAccess,
    },
};

fn terminal_agent(artifacts: Vec<ArtifactReference>) -> WorkflowNodeSnapshot {
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
        work: "review the change".into(),
        state: NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        },
        current_attempt: None,
        command_exit: None,
        validated_output: None,
        artifacts,
        terminal_reason: None,
    }
}

fn artifact(kind: ArtifactKind, relative_path: &str, bytes: &[u8]) -> ArtifactReference {
    ArtifactReference {
        kind,
        artifact: ArtifactRef {
            relative_path: relative_path.into(),
            retained_bytes: bytes.len() as u64,
            observed: ArtifactObservation::Complete {
                observed_bytes: bytes.len() as u64,
            },
            digest: Digest(format!("sha256:{:0>64}", bytes.len())),
        },
    }
}

fn write_private(run_dir: &Path, relative: &str, bytes: &[u8]) {
    write_file_beneath(run_dir, Path::new(relative), bytes).expect("private artifact");
}

// Covers: finished agent details prefer the answer artifact over streams.
// Owner: workflow TUI output projection.
#[test]
fn prefers_agent_answer_over_stdout() {
    let node = terminal_agent(vec![
        artifact(ArtifactKind::Stdout, "artifacts/review/stdout", b"log"),
        artifact(
            ArtifactKind::AgentAnswer,
            "artifacts/review/answer.txt",
            b"# ok",
        ),
    ]);
    let primary = primary_artifact(&node).expect("primary");
    assert_eq!(primary.kind, ArtifactKind::AgentAnswer);
    assert_eq!(kind_label(primary.kind), "answer");
}

// Covers: selecting a finished node loads and formats its durable answer.
// Owner: workflow TUI output projection.
#[test]
fn loads_and_renders_markdown_answer() {
    let dir = tempdir().unwrap();
    let relative = "artifacts/review/answer.txt";
    let bytes = b"## Findings\n\n- risk low\n";
    write_private(dir.path(), relative, bytes);

    let node = terminal_agent(vec![artifact(ArtifactKind::AgentAnswer, relative, bytes)]);
    let body = load_finished_output(dir.path(), &node).expect("body");
    assert!(body_matches_node(&body, &node));
    assert!(body.notice.is_none());
    let lines = render_body_lines(&body, 40);
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
    assert!(text.contains("Findings"));
    assert!(text.contains("risk low"));
}

// Covers: non-private artifact files fail closed instead of plain open fallback.
// Owner: workflow TUI output projection.
#[cfg(unix)]
#[test]
fn rejects_world_readable_artifact_files() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempdir().unwrap();
    let relative = "artifacts/review/answer.txt";
    let path = dir.path().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"leaked").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let node = terminal_agent(vec![artifact(
        ArtifactKind::AgentAnswer,
        relative,
        b"leaked",
    )]);
    let body = load_finished_output(dir.path(), &node).expect("body shell");
    assert!(body.text.is_empty());
    assert!(
        body.notice
            .as_deref()
            .is_some_and(|notice| notice.contains("could not open output")),
        "notice={:?}",
        body.notice
    );
}

// Covers: invalid UTF-8 is shown lossily with one explicit notice.
// Owner: workflow TUI output projection.
#[test]
fn invalid_utf8_gets_a_single_notice() {
    let dir = tempdir().unwrap();
    let relative = "artifacts/review/stdout";
    let bytes = b"ok\xffmore";
    write_private(dir.path(), relative, bytes);

    let node = terminal_agent(vec![artifact(ArtifactKind::Stdout, relative, bytes)]);
    let body = load_finished_output(dir.path(), &node).expect("body");
    assert!(body.text.contains("ok"));
    assert!(body.text.contains("more"));
    assert_eq!(
        body.notice.as_deref(),
        Some("output is not valid UTF-8; showing lossy text")
    );
}

// Covers: running nodes do not load output bodies yet.
// Owner: workflow TUI output projection.
#[test]
fn skips_running_nodes() {
    let dir = tempdir().unwrap();
    let mut node = terminal_agent(vec![artifact(
        ArtifactKind::AgentAnswer,
        "artifacts/review/answer.txt",
        b"secret",
    )]);
    node.state = NodeState::Running {
        attempt: crate::workflow::AttemptNumber::new(1).unwrap(),
    };
    assert!(load_finished_output(dir.path(), &node).is_none());
}
