use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    durable_artifacts_for_node, effective_permission_mode_for, runtime_event_json,
    WORKFLOW_WIRE_VERSION,
};
use crate::{
    app::workflow_runtime::RuntimeEvent,
    permission::PermissionMode,
    workflow::{AttemptNumber, NodeId, NodeTerminalState, RunId},
};

// Covers: every executor in a workflow run must use one mode no broader than
// either current policy or any frozen agent ceiling.
// Owner: workflow runtime authorization composition.
#[test]
fn effective_mode_is_the_narrowest_run_wide_ceiling() {
    for (current, frozen, expected) in [
        (PermissionMode::Auto, &[][..], PermissionMode::Auto),
        (
            PermissionMode::Auto,
            &["supervised", "auto"][..],
            PermissionMode::Supervised,
        ),
        (
            PermissionMode::Supervised,
            &["auto", "plan"][..],
            PermissionMode::Plan,
        ),
        (
            PermissionMode::Plan,
            &["auto", "supervised"][..],
            PermissionMode::Plan,
        ),
    ] {
        assert_eq!(
            effective_permission_mode_for(current, frozen.iter().copied()).unwrap(),
            expected
        );
    }
    assert!(effective_permission_mode_for(PermissionMode::Auto, ["invalid"]).is_err());
}

// Covers: the TUI must show the typed artifact metadata stored in the durable completion.
// Owner: workflow CLI to TUI projection.
#[test]
fn tui_artifacts_come_from_durable_completions() {
    let workflow =
        crate::workflow::test_support::workflow(vec![crate::workflow::test_support::agent_node(
            "inspect",
            &[],
            crate::workflow::WorkspaceAccess::ReadOnly,
        )]);
    let id = crate::workflow::test_support::id("inspect");
    let mut state = crate::workflow::test_support::state(&workflow);
    let artifact = crate::workflow::ArtifactRef {
        relative_path: "nodes/inspect/attempts/1/agent/answer.txt".into(),
        retained_bytes: 8,
        observed: crate::workflow::ArtifactObservation::Truncated {
            observed_bytes_at_least: 13,
        },
        digest: crate::workflow::Digest("sha256:test".into()),
    };
    state.completions.insert(
        id.clone(),
        crate::workflow::NodeCompletion {
            attempt: Some(crate::workflow::AttemptNumber::new(1).unwrap()),
            outcome: crate::workflow::NodeTerminalState::Success,
            cancellation_resume: None,
            command_exit: None,
            structured_output: None,
            artifacts: crate::workflow::AttemptArtifacts {
                answer: Some(artifact.clone()),
                ..crate::workflow::AttemptArtifacts::default()
            },
        },
    );

    assert_eq!(
        durable_artifacts_for_node(&state, &id),
        vec![crate::tui::workflow::ArtifactReference {
            kind: crate::workflow::ArtifactKind::AgentAnswer,
            artifact,
        }]
    );
}

// Covers: RuntimeEvent wire body comes from Serialize, with envelope fields layered on.
// Owner: workflow CLI runtime event presentation.
#[test]
fn runtime_event_json_matches_wire_shape() {
    let run_id = "00000000-0000-4000-8000-000000000001"
        .parse::<RunId>()
        .unwrap();
    let node = NodeId::new("build").unwrap();
    let attempt = AttemptNumber::new(2).unwrap();

    assert_eq!(
        runtime_event_json(1, run_id, &RuntimeEvent::StateChanged { revision: 7 }),
        json!({
            "type": "state_changed",
            "revision": 7,
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 1,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(
            2,
            run_id,
            &RuntimeEvent::NodeStarted {
                node: node.clone(),
                attempt
            }
        ),
        json!({
            "type": "node_started",
            "node": "build",
            "attempt": 2,
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 2,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(
            3,
            run_id,
            &RuntimeEvent::NodeFinished {
                node: node.clone(),
                outcome: NodeTerminalState::Success
            }
        ),
        json!({
            "type": "node_finished",
            "node": "build",
            "outcome": "success",
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 3,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(
            4,
            run_id,
            &RuntimeEvent::NeedsRecovery {
                nodes: vec![node.clone()]
            }
        ),
        json!({
            "type": "needs_recovery",
            "nodes": ["build"],
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 4,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(5, run_id, &RuntimeEvent::Completed),
        json!({
            "type": "completed",
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 5,
            "run_id": run_id.to_string(),
        })
    );
}

// Covers: tools and CLI text share one RuntimeEvent message path.
// Owner: workflow CLI runtime event presentation.
#[test]
fn runtime_event_message_is_canonical() {
    let node = NodeId::new("build").unwrap();
    let attempt = AttemptNumber::new(2).unwrap();
    assert_eq!(
        RuntimeEvent::StateChanged { revision: 3 }.message(),
        "workflow state revision 3"
    );
    assert_eq!(
        RuntimeEvent::NodeStarted {
            node: node.clone(),
            attempt
        }
        .message(),
        "workflow node build started attempt 2"
    );
    assert_eq!(
        RuntimeEvent::NodeFinished {
            node: node.clone(),
            outcome: NodeTerminalState::Failure
        }
        .message(),
        "workflow node build finished: Failure"
    );
    assert_eq!(
        RuntimeEvent::NeedsRecovery {
            nodes: vec![node, NodeId::new("test").unwrap()]
        }
        .message(),
        "workflow needs recovery: build, test"
    );
    assert_eq!(RuntimeEvent::Completed.message(), "workflow completed");
}
