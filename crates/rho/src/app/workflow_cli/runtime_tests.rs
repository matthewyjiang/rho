use pretty_assertions::assert_eq;

use super::{durable_artifacts_for_node, effective_permission_mode_for};
use crate::permission::PermissionMode;

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
