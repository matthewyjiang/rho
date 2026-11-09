use pretty_assertions::assert_eq;

use super::{
    drive_end, effective_permission_mode_for, log_drive_end, WorkflowApprovalMode, WorkflowRuntime,
};
use crate::{
    app::{subagent_pool::WorkflowEnd, workflow_runtime::RuntimeError},
    permission::PermissionMode,
    workflow::{
        self, PlanConsent, PlanId, RunId, RunLifecycle, RunManifest, RunStateRecord, ScopeResult,
        StoredRun, WorkflowOutcome,
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
            PermissionMode::Auto,
            &["allow_edits", "auto"][..],
            PermissionMode::AllowEdits,
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
            program_digest: graph.program_digest.clone(),
            workspace_identity: "test-workspace".into(),
            consent: PlanConsent {
                program_digest: graph.program_digest.clone(),
                confirmed: true,
            },
            name: "test".into(),
            step_count: graph.program.root.nodes.len(),
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

    let workspace = tempfile::tempdir().unwrap();
    let error = match WorkflowRuntime::build(
        &run,
        workspace.path(),
        Some(config.configured_path().unwrap()),
        WorkflowApprovalMode::non_interactive(Default::default()),
        crate::app::subagent_pool::SubagentPool::Own,
        crate::session_environment::Ambient::ThisProcess,
    ) {
        Ok(_) => panic!("Auto workflow startup must require the classifier model"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "permission mode auto requires a configured permission-classifier model (set it with /agents, /config in a daemon TUI, or config.toml [internal_agents.permission-classifier])"
    );
}

/// A run whose drive left it `lifecycle`, with root `outcome` once
/// completed.
fn ended_run(lifecycle: RunLifecycle, outcome: Option<WorkflowOutcome>) -> StoredRun {
    let mut run = stored_run(workflow::test_support::workflow(Vec::new()));
    run.state.state.lifecycle = lifecycle;
    let root = run
        .state
        .state
        .scopes
        .values_mut()
        .next()
        .expect("a run has its root scope");
    root.result = outcome.map(|outcome| ScopeResult {
        outcome,
        outputs: Default::default(),
    });
    run
}

// Covers: a background drive tells its host the run succeeded only when the
// run's outcome is success; cancellation is cancelled; a failed, denied, or
// blocked run, a run left needing recovery, and a drive that errored are
// failed, saying why.
// Owner: workflow background runs (the end a hosting daemon records).
#[test]
fn a_drive_reports_how_its_run_ended() {
    use RunLifecycle::{Completed, NeedsRecovery};
    let failed = |reason: &str| WorkflowEnd::Failed(reason.into());
    let cases = [
        (
            Ok(ended_run(Completed, Some(WorkflowOutcome::Success))),
            WorkflowEnd::Completed,
        ),
        (
            Ok(ended_run(Completed, Some(WorkflowOutcome::Cancellation))),
            WorkflowEnd::Cancelled,
        ),
        (
            Ok(ended_run(Completed, Some(WorkflowOutcome::Denial))),
            failed("the workflow run ended with denial"),
        ),
        (
            Ok(ended_run(Completed, Some(WorkflowOutcome::Blocked))),
            failed("the workflow run ended with blocked"),
        ),
        (
            Ok(ended_run(NeedsRecovery, None)),
            failed("the workflow run stopped in state needs_recovery"),
        ),
        (
            Err(RuntimeError::Data("the journal is torn".into())),
            failed("workflow runtime data is invalid: the journal is torn"),
        ),
    ];
    for (result, expected) in cases {
        assert_eq!(drive_end(&result), expected, "{result:?}");
    }
}

/// What `log` emits, rendered in the daemon's journal sink format, one line
/// per event.
fn journal_lines(log: impl FnOnce()) -> String {
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = crate::logging::journal_format(tracing_subscriber::fmt())
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, log);
    let bytes = captured.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

// Covers: the daemon log line for a background run names how it ended; a
// failed, denied, or blocked run logs as failed with its outcome instead of
// completed (#1736).
// Owner: workflow background runs (the daemon's log of a run's end).
#[test]
fn a_background_run_logs_its_real_outcome() {
    let cases = [
        (
            WorkflowOutcome::Success,
            " INFO rho_coding_agent::app::workflow_cli::runtime: background workflow completed",
            "",
        ),
        (
            WorkflowOutcome::Cancellation,
            " INFO rho_coding_agent::app::workflow_cli::runtime: background workflow cancelled",
            "",
        ),
        (
            WorkflowOutcome::Failure,
            " WARN rho_coding_agent::app::workflow_cli::runtime: background workflow failed",
            " error=the workflow run ended with failure",
        ),
    ];
    for (outcome, event, reason) in cases {
        let run = ended_run(RunLifecycle::Completed, Some(outcome));
        let run_id = run.manifest.run_id;
        let end = drive_end(&Ok(run));

        assert_eq!(
            journal_lines(|| log_drive_end(run_id, &end)),
            format!("{event} run_id={run_id}{reason}\n"),
            "{outcome:?}"
        );
    }
}
