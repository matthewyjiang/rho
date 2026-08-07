use super::*;
use crate::workflow::RunLifecycle;

// Covers: malformed or duplicate --input values could produce an ambiguous frozen plan.
// Owner: workflow CLI input parser.
#[test]
fn parses_inputs_as_typed_values_and_rejects_ambiguous_entries() {
    let parsed = parse_inputs(&[
        "target=\"src\"".to_owned(),
        "attempts=3".to_owned(),
        "enabled=true".to_owned(),
    ])
    .unwrap();
    assert_eq!(
        parsed,
        BTreeMap::from([
            (
                InputName::new("attempts").unwrap(),
                WorkflowValue::Integer(3),
            ),
            (
                InputName::new("enabled").unwrap(),
                WorkflowValue::Bool(true),
            ),
            (
                InputName::new("target").unwrap(),
                WorkflowValue::String("src".to_owned()),
            ),
        ])
    );

    for values in [
        vec!["missing-separator".to_owned()],
        vec!["target=not-json".to_owned()],
        vec!["target=1".to_owned(), "target=2".to_owned()],
    ] {
        assert!(parse_inputs(&values).is_err());
    }
}

// Covers: a redirected run could start without consent, or --yes could still prompt.
// Owner: workflow CLI confirmation policy.
#[test]
fn confirmation_policy_requires_yes_only_when_not_interactive() {
    assert_eq!(
        confirmation_requirement(true, false),
        ConfirmationRequirement::Confirmed
    );
    assert_eq!(
        confirmation_requirement(true, true),
        ConfirmationRequirement::Confirmed
    );
    assert_eq!(
        confirmation_requirement(false, true),
        ConfirmationRequirement::Prompt
    );
    assert_eq!(
        confirmation_requirement(false, false),
        ConfirmationRequirement::FlagRequired
    );
}

// Covers: model workflow diagnostics must not expose local paths, while local
// CLI diagnostics keep the full path needed to fix the error.
// Owner: workflow CLI diagnostic adapter.
#[test]
fn model_diagnostics_redact_workflow_error_paths() {
    let private_path = PathBuf::from("/home/alice/private/workflow.star");
    let reason_path = "C:\\Users\\alice\\private\\state.json";
    let cases = [
        (
            WorkflowError::SourceOutsideRoot {
                path: private_path.clone(),
            },
            "workflow source path is outside module root: <redacted>",
        ),
        (
            WorkflowError::SourceSymlink {
                path: private_path.clone(),
            },
            "workflow source path contains a symlink: <redacted>",
        ),
        (
            WorkflowError::Corrupt {
                path: private_path.clone(),
                reason: format!("failed to read {reason_path}"),
            },
            "workflow data is corrupt at <redacted>: <redacted>",
        ),
        (
            WorkflowError::UntrustedDirectory(private_path.clone()),
            "workflow store boundary is not a trusted directory: <redacted>",
        ),
    ];

    for (error, expected_model_message) in cases {
        let full_message = error.to_string();
        let error = anyhow::Error::from(error);
        assert_eq!(diagnostic_for_error(&error).message, full_message);
        assert_eq!(
            diagnostic_for_model_error(&error).message,
            expected_model_message
        );
    }

    let error = anyhow::Error::from(WorkflowError::MissingWorkflow);
    assert_eq!(
        diagnostic_for_model_error(&error).message,
        diagnostic_for_error(&error).message
    );
}

// Covers: anyhow context must not bypass model path redaction when the owned
// WorkflowError is lower in the error chain.
// Owner: workflow CLI diagnostic adapter.
#[test]
fn model_diagnostics_redact_nested_workflow_errors() {
    let data_path = PathBuf::from("/home/alice/private/run/state.json");
    let reason_path = "/mnt/secret/run-events.jsonl";
    let error = anyhow::Error::from(WorkflowError::Corrupt {
        path: data_path.clone(),
        reason: format!("nested read failed at {reason_path}"),
    })
    .context(format!("failed to load {}", data_path.display()));

    assert_eq!(
        diagnostic_for_model_error(&error).message,
        "workflow data is corrupt at <redacted>: <redacted>"
    );
    let local_message = diagnostic_for_error(&error).message;
    assert!(local_message.contains(data_path.to_str().unwrap()));
    assert!(local_message.contains(reason_path));
}

// Covers: an oversized worker frame must fail before allocating or deserializing its body.
// Owner: supervised planner IPC framing.
#[test]
fn planner_frame_rejects_zero_and_oversized_lengths() {
    for requested in [0, PLANNER_REQUEST_FRAME_BYTES + 1] {
        let error = read_frame_sync(
            std::io::Cursor::new(requested.to_be_bytes()),
            PLANNER_REQUEST_FRAME_BYTES,
        )
        .unwrap_err();
        assert!(error.downcast_ref::<WorkflowError>().is_some());
    }
}

// Covers: cancel must expose a typed pending result at its measured wait bound
// instead of treating an older acknowledgement as success.
// Owner: workflow CLI cancellation contract.
#[tokio::test]
async fn cancellation_wait_returns_typed_exact_state() {
    let mut checks = 0;
    let acknowledged = wait_for_cancellation_ack(
        || {
            checks += 1;
            Ok::<_, std::convert::Infallible>(true)
        },
        Duration::ZERO,
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert!(acknowledged);
    assert_eq!(
        cancellation_state(acknowledged, RunLifecycle::Running),
        CancellationState::Acknowledged
    );
    assert_eq!(checks, 1);

    let pending = wait_for_cancellation_ack(
        || Ok::<_, std::convert::Infallible>(false),
        Duration::ZERO,
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert!(!pending);
    assert_eq!(
        cancellation_state(pending, RunLifecycle::Running),
        CancellationState::Pending
    );
    assert_eq!(
        cancellation_state(true, RunLifecycle::Completed),
        CancellationState::Acknowledged
    );
    assert_eq!(
        cancellation_state(false, RunLifecycle::Completed),
        CancellationState::AlreadyCompleted
    );
}

// Covers: resume must use the run's copied graph when its plan and source no longer exist.
// Owner: workflow CLI resume preflight.
#[tokio::test]
async fn resume_preflight_is_self_contained_after_plan_deletion() {
    struct SuccessfulExecutor;
    impl crate::app::workflow_runtime::WorkflowNodeExecutor for SuccessfulExecutor {
        fn execute<'a>(
            &'a self,
            _request: crate::app::workflow_runtime::NodeExecutionRequest,
        ) -> crate::app::workflow_runtime::WorkflowExecutionFuture<'a> {
            Box::pin(async {
                Ok(crate::app::workflow_runtime::NodeExecutionResult::terminal(
                    crate::workflow::NodeTerminalState::Success,
                ))
            })
        }
    }

    let home = tempfile::tempdir().unwrap();
    let source = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(source.path(), "WORKFLOW = None").unwrap();
    let workspace = std::env::current_dir().unwrap().canonicalize().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let graph =
        crate::workflow::test_support::workflow(vec![crate::workflow::test_support::agent_node(
            "inspect",
            &[],
            crate::workflow::WorkspaceAccess::Mutating,
        )]);
    let plan = store
        .create_plan(
            &graph,
            crate::paths::display(&workspace),
            &BTreeMap::from([("//workflow.star".to_owned(), "WORKFLOW = None".to_owned())]),
        )
        .unwrap();
    let run = store
        .create_run(
            &plan,
            PlanConsent {
                graph_digest: plan.manifest.graph_digest.clone(),
                confirmed: true,
            },
            crate::workflow::RunStateRecord {
                schema_version: crate::workflow::RUN_STATE_VERSION,
                last_event_sequence: 0,
                state: crate::workflow::WorkflowState {
                    lifecycle: crate::workflow::RunLifecycle::Planned,
                    ..crate::workflow::test_support::state(&graph)
                },
            },
        )
        .unwrap();
    std::fs::remove_dir_all(
        home.path()
            .join("workflows/plans")
            .join(plan.manifest.plan_id.to_string()),
    )
    .unwrap();
    std::fs::remove_file(source.path()).unwrap();
    let config = super::ConfigRepository::temporary_for_tests().unwrap();

    // recheck_run validates the in-memory run graph and workspace identity; it does not
    // need the temporary store root. Ops still need a creatable rho home on fresh hosts.
    WorkflowOps::open(
        std::env::current_dir().unwrap(),
        Some(config.configured_path().unwrap()),
    )
    .unwrap()
    .recheck_run(&run)
    .unwrap();
    let executor: Arc<dyn crate::app::workflow_runtime::WorkflowNodeExecutor> =
        Arc::new(SuccessfulExecutor);
    let resumed = WorkflowRunner::new(
        home.path().to_owned(),
        workspace,
        crate::app::workflow_runtime::RuntimeSecurity {
            project_trusted: true,
            permission_mode: crate::permission::PermissionMode::Auto,
        },
        Arc::clone(&executor),
        executor,
    )
    .drive(
        run.manifest.run_id,
        crate::app::workflow_runtime::RecoveryDecision::NormalResume,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        resumed.state.state.lifecycle,
        crate::workflow::RunLifecycle::Completed
    );
}
