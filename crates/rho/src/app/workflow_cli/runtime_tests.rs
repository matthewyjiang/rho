use pretty_assertions::assert_eq;

use super::{effective_permission_mode_for, WorkflowApprovalMode, WorkflowRuntime};
use crate::{
    permission::PermissionMode,
    workflow::{
        self, PlanConsent, PlanId, RunId, RunLifecycle, RunManifest, RunStateRecord, StoredRun,
    },
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

fn stored_run(graph: workflow::FrozenWorkflow) -> StoredRun {
    StoredRun {
        manifest: RunManifest {
            schema_version: workflow::RUN_MANIFEST_VERSION,
            run_id: RunId::new(),
            created_at_unix_nanos: 0,
            plan_id: PlanId::new(),
            graph_digest: graph.graph_digest.clone(),
            workspace_identity: "test-workspace".into(),
            consent: PlanConsent {
                graph_digest: graph.graph_digest.clone(),
                confirmed: true,
            },
            name: "test".into(),
            step_count: graph.graph.nodes.len(),
        },
        state: RunStateRecord {
            schema_version: workflow::RUN_STATE_VERSION,
            last_event_sequence: 0,
            state: workflow::WorkflowState {
                lifecycle: RunLifecycle::Planned,
                ..workflow::test_support::state(&graph)
            },
        },
        graph,
    }
}

// Covers: workflow startup in Auto mode must fail before running commands when
// the permission classifier model is not configured.
// Owner: workflow runtime authorization composition.
#[test]
fn auto_workflow_requires_configured_permission_classifier_model() {
    let config = crate::app::config_repository::ConfigRepository::temporary_for_tests().unwrap();
    config
        .update(|config| config.permission_mode = PermissionMode::Auto)
        .unwrap();
    let run = stored_run(workflow::test_support::workflow(Vec::new()));

    let error = match WorkflowRuntime::build(
        &run,
        Some(config.configured_path().unwrap()),
        WorkflowApprovalMode::non_interactive(Default::default()),
    ) {
        Ok(_) => panic!("Auto workflow startup must require the classifier model"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "permission mode auto requires a configured permission-classifier model (set via /config or config.toml [internal_agents.permission-classifier])"
    );
}
