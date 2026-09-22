use super::{test_support::*, *};
use crate::workflow::*;
use pretty_assertions::assert_eq;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

// Covers: a result must become a durable terminal state and attempt record.
// Owner: durable workflow runner.
#[tokio::test]
async fn runner_persists_successful_attempt() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let completed = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor))
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();
    assert_eq!(completed.state.state.lifecycle, RunLifecycle::Completed);
    assert_eq!(
        terminal(&completed, "inspect"),
        Some(NodeTerminalState::Success)
    );
    let attempt = AttemptNumber::new(1).unwrap();
    let record = journal::read_attempt_record(
        &WorkflowLayout::new(home.path()).run(run.manifest.run_id),
        &task_id("inspect"),
        attempt,
    )
    .unwrap();
    assert_eq!(
        record.state,
        AttemptState::Completed {
            completion: Box::new(NodeCompletion {
                attempt: Some(attempt),
                ..NodeCompletion::terminal(NodeTerminalState::Success)
            })
        }
    );
}

// Covers: preparation errors previously disappeared into a bare Failure outcome.
// Owner: runtime driver; pure binding tests do not exercise durable failure reporting.
#[tokio::test]
async fn preparation_failure_is_durable_and_never_dispatches() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut graph = test_workflow();
    // The frozen prompt is valid, but binding its output-schema instructions exceeds
    // the runtime expansion budget. This reaches the real prepare path after load.
    let NodeExecution::Agent(agent) = &mut graph
        .program
        .root
        .nodes
        .get_mut(&node_id("inspect"))
        .unwrap()
        .execution
    else {
        unreachable!()
    };
    agent.output = Some(OutputSchema::Bool);
    graph.runtime_limits.prompt_expansion_bytes = "review".len() as u64;
    graph.program_digest = program_digest(&graph).unwrap();
    let run = create_run_with_workflow(home.path(), workspace.path(), graph);
    let expected =
        prepared::PreparedInvocation::prepare(&run.graph, &task_id("inspect"), &run.state.state)
            .err()
            .unwrap()
            .to_string();
    let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let completed = runner(home.path(), workspace.path(), executor.clone())
        .drive(
            run.manifest.run_id,
            RecoveryDecision::NormalResume,
            Some(sender),
        )
        .await
        .unwrap();
    assert_eq!(executor.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        terminal(&completed, "inspect"),
        Some(NodeTerminalState::Failure)
    );
    let completion = completion(&completed, "inspect");
    assert_eq!(completion.attempt, Some(AttemptNumber::new(1).unwrap()));
    let directory = WorkflowLayout::new(home.path()).run(run.manifest.run_id);
    let diagnostic = completion.artifacts.answer.as_ref().unwrap();
    assert_eq!(
        std::fs::read_to_string(directory.join(&diagnostic.relative_path)).unwrap(),
        expected
    );
    let mut reported = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let RuntimeEvent::NodeProgress { message, .. } = event {
            reported.push(message);
        }
    }
    assert_eq!(reported, vec![expected]);
    let loaded = WorkflowStore::new(home.path())
        .unwrap()
        .load_run(run.manifest.run_id)
        .unwrap();
    assert_eq!(loaded.state, completed.state);
    assert_replay(home.path(), &completed);
    let resumed = runner(home.path(), workspace.path(), executor.clone())
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();
    assert_eq!(resumed.state, completed.state);
    assert_eq!(executor.0.load(Ordering::SeqCst), 0);
}

// Covers: mutable attempt status cannot substitute for the journal completion.
// Owner: workflow journal completion binding.
#[tokio::test]
async fn attempt_status_substitution_cannot_change_journal_completion() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let completed = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor))
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();
    let attempt = AttemptNumber::new(1).unwrap();
    let status = attempt_directory(
        &WorkflowLayout::new(home.path()).run(run.manifest.run_id),
        &task_id("inspect"),
        attempt,
    )
    .join("status.json");
    std::fs::write(
        status,
        serde_json::to_vec(&AttemptRecord {
            schema_version: ATTEMPT_VERSION,
            attempt,
            state: AttemptState::Completed {
                completion: Box::new(NodeCompletion {
                    attempt: Some(attempt),
                    ..NodeCompletion::terminal(NodeTerminalState::Failure)
                }),
            },
        })
        .unwrap(),
    )
    .unwrap();
    let loaded = WorkflowStore::new(home.path())
        .unwrap()
        .load_run(run.manifest.run_id)
        .unwrap();
    assert_eq!(loaded.state, completed.state);
    let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
    let resumed = runner(home.path(), workspace.path(), executor.clone())
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();
    assert_eq!(executor.0.load(Ordering::SeqCst), 0);
    assert_eq!(resumed.state, completed.state);
}

// Covers: artifact writes must not follow substituted parent or final symlinks.
// Owner: durable workflow artifact storage.
#[cfg(unix)]
#[test]
fn artifact_writes_do_not_follow_symlinks() {
    use std::os::unix::fs::symlink;
    let run = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let attempt = attempt_directory(
        run.path(),
        &task_id("inspect"),
        AttemptNumber::new(1).unwrap(),
    );
    ensure_directory_beneath(run.path(), attempt.strip_prefix(run.path()).unwrap()).unwrap();
    let agent = attempt.join("agent");
    symlink(outside.path(), &agent).unwrap();
    assert!(artifacts::write_artifact(run.path(), &agent.join("answer.txt"), b"blocked").is_err());
    std::fs::remove_file(&agent).unwrap();
    std::fs::create_dir(&agent).unwrap();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, b"outside").unwrap();
    let answer = agent.join("answer.txt");
    symlink(&outside_file, &answer).unwrap();
    artifacts::write_artifact(run.path(), &answer, b"inside").unwrap();
    assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside");
    assert_eq!(std::fs::read(&answer).unwrap(), b"inside");
    assert!(!std::fs::symlink_metadata(&answer)
        .unwrap()
        .file_type()
        .is_symlink());
}

// Covers: separate CLI cancellation must acknowledge its receipt and resume cleanly.
// Owner: durable workflow runner cancellation.
#[tokio::test(flavor = "current_thread")]
async fn cross_process_request_cancels_active_node() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run_with_workflow(home.path(), workspace.path(), cancellation_workflow());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let active_runner = Arc::new(runner(
        home.path(),
        workspace.path(),
        Arc::new(CancellationExecutor {
            started: std::sync::Mutex::new(Some(started_tx)),
        }),
    ));
    let worker_runner = Arc::clone(&active_runner);
    let run_id = run.manifest.run_id;
    let worker = tokio::spawn(async move {
        worker_runner
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await
    });
    within_budget("cross-process worker start", started_rx)
        .await
        .unwrap();
    let receipt = request_cross_process_cancel(home.path(), run_id).unwrap();
    active_runner.wake_cancel_check();
    let completed = join_within_budget("cross-process cancellation", worker)
        .await
        .unwrap();
    assert_eq!(completed.state.state.lifecycle, RunLifecycle::Completed);
    assert!(completed.state.state.cancellation_requested);
    for node in ["inspect", "report"] {
        assert_eq!(
            terminal(&completed, node),
            Some(NodeTerminalState::Cancellation)
        );
    }
    assert_eq!(
        completed.state.state.outcome(),
        Some(WorkflowOutcome::Cancellation)
    );
    assert!(completed
        .state
        .state
        .tasks()
        .all(|(_, state)| state.terminal().is_some()));
    assert!(cross_process_cancel_acknowledged(home.path(), run_id).unwrap());
    assert!(cancellation_request_acknowledged(home.path(), run_id, &receipt).unwrap());
    let resumed = within_budget(
        "cross-process resume after cancel",
        runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor)).drive(
            run_id,
            RecoveryDecision::NormalResume,
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        resumed.state.state.outcome(),
        Some(WorkflowOutcome::Success)
    );
    assert_eq!(
        completion(&resumed, "inspect").attempt,
        Some(AttemptNumber::new(2).unwrap())
    );
    assert_eq!(
        completion(&resumed, "report").attempt,
        Some(AttemptNumber::new(1).unwrap())
    );
    assert!(resumed
        .state
        .state
        .tasks()
        .all(|(_, state)| state.terminal() == Some(NodeTerminalState::Success)));
    assert_replay(home.path(), &resumed);
    let unobserved = request_cross_process_cancel(home.path(), run_id).unwrap();
    assert!(!cancellation_request_acknowledged(home.path(), run_id, &unobserved).unwrap());
}

// Covers: concurrent retries must share an acknowledgeable durable receipt.
// Owner: durable workflow cancellation request creation.
#[tokio::test(flavor = "current_thread")]
async fn concurrent_cancellation_requests_share_the_active_receipt() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let request = |barrier: Arc<std::sync::Barrier>| {
        let home = home.path().to_owned();
        let run_id = run.manifest.run_id;
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            request_cross_process_cancel(&home, run_id).unwrap()
        })
    };
    let first = request(Arc::clone(&barrier));
    let second = request(Arc::clone(&barrier));
    barrier.wait();
    let (first, second) = within_budget("concurrent cancellation receipts", async {
        tokio::join!(first, second)
    })
    .await;
    assert_eq!(first.unwrap(), second.unwrap());
}

// Covers: uncertain cleanup retains exclusion until cleanup ends and cannot
// acknowledge cancellation until the caller confirms no process remains.
// Owner: durable workflow agent cancellation cleanup.
#[tokio::test(flavor = "current_thread")]
async fn uncertain_agent_cleanup_is_durable_recoverable_and_keeps_locks() {
    for (reason, expected_cause) in [
        (
            agent::AgentStopReason::Cancellation,
            CleanupCause::Cancellation,
        ),
        (agent::AgentStopReason::Timeout, CleanupCause::Timeout),
    ] {
        let mut never_completes = NeverCompletingAgentExecutor {
            cancelled: AtomicBool::new(false),
        };
        let cleanup =
            agent::stop_agent(&mut never_completes, reason, std::time::Duration::ZERO).await;
        assert!(
            matches!(cleanup, Err(RuntimeError::CleanupUncertain { cause }) if cause == expected_cause)
        );
        assert!(never_completes.cancelled.load(Ordering::SeqCst));
    }
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let competing_run = create_run(home.path(), workspace.path());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (cleanup_started_tx, cleanup_started_rx) = tokio::sync::oneshot::channel();
    let (release_cleanup_tx, release_cleanup_rx) = tokio::sync::oneshot::channel();
    let executor = Arc::new(UncertainCleanupExecutor {
        started: std::sync::Mutex::new(Some(started_tx)),
        cleanup_started: std::sync::Mutex::new(Some(cleanup_started_tx)),
        release_cleanup: std::sync::Mutex::new(Some(release_cleanup_rx)),
    });
    let active_runner = Arc::new(runner(home.path(), workspace.path(), executor));
    let worker_runner = Arc::clone(&active_runner);
    let run_id = run.manifest.run_id;
    let worker = tokio::spawn(async move {
        worker_runner
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await
    });
    within_budget("uncertain cleanup worker start", started_rx)
        .await
        .unwrap();
    let receipt = active_runner
        .cancellation_request(run_id)
        .request()
        .unwrap();
    within_budget("uncertain cleanup started", cleanup_started_rx)
        .await
        .unwrap();
    assert_eq!(
        request_cross_process_cancel(home.path(), run_id).unwrap(),
        receipt
    );
    assert!(matches!(
        within_budget(
            "active-owner check while cleanup holds lock",
            active_runner.drive(run_id, RecoveryDecision::NormalResume, None)
        )
        .await,
        Err(RuntimeError::ActiveOwner)
    ));
    let (competing_executor_tx, mut competing_executor_rx) = tokio::sync::oneshot::channel();
    let competing_executor = Arc::new(SignallingSuccessfulExecutor {
        started: std::sync::Mutex::new(Some(competing_executor_tx)),
    });
    let competing_runner = Arc::new(runner(home.path(), workspace.path(), competing_executor));
    let (competing_events_tx, mut competing_events_rx) = tokio::sync::mpsc::unbounded_channel();
    let competing_worker = tokio::spawn(async move {
        competing_runner
            .drive(
                competing_run.manifest.run_id,
                RecoveryDecision::NormalResume,
                Some(competing_events_tx),
            )
            .await
    });
    loop {
        match within_budget("competing run start events", competing_events_rx.recv()).await {
            Some(RuntimeEvent::NodeStarted { .. }) => break,
            Some(_) => {}
            None => panic!("competing run ended before it started a node"),
        }
    }
    assert!(matches!(
        competing_executor_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    release_cleanup_tx
        .send(())
        .expect("cleanup worker still waiting for release");
    let worker_result = join_within_budget("uncertain cleanup worker", worker).await;
    assert!(
        matches!(worker_result, Err(RuntimeError::NeedsRecovery { .. })),
        "unexpected cleanup result: {worker_result:?}"
    );
    within_budget("competing executor start", competing_executor_rx)
        .await
        .unwrap();
    assert_eq!(
        join_within_budget("competing worker", competing_worker)
            .await
            .unwrap()
            .state
            .state
            .outcome(),
        Some(WorkflowOutcome::Success)
    );
    let store = WorkflowStore::new(home.path()).unwrap();
    let uncertain = store.load_run(run_id).unwrap();
    assert_eq!(uncertain.state.state.lifecycle, RunLifecycle::NeedsRecovery);
    assert_eq!(
        uncertain.state.state.task(&task_id("inspect")),
        Some(&NodeState::Running {
            attempt: AttemptNumber::new(1).unwrap()
        })
    );
    let attempt = journal::read_attempt_record(
        &WorkflowLayout::new(home.path()).run(run_id),
        &task_id("inspect"),
        AttemptNumber::new(1).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        attempt.state,
        AttemptState::InterruptedUncertain { .. }
    ));
    assert!(!cancellation_request_acknowledged(home.path(), run_id, &receipt).unwrap());
    assert!(matches!(
        within_budget(
            "normal resume rejected while uncertain",
            runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor)).drive(
                run_id,
                RecoveryDecision::NormalResume,
                None
            )
        )
        .await,
        Err(RuntimeError::NeedsRecovery { .. })
    ));
    let resumed = within_budget(
        "confirm-no-process resume after uncertain cleanup",
        runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor)).drive(
            run_id,
            RecoveryDecision::ConfirmNoProcess,
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        resumed.state.state.outcome(),
        Some(WorkflowOutcome::Success)
    );
    assert!(cancellation_request_acknowledged(home.path(), run_id, &receipt).unwrap());
}

// Covers: uncertain timeout cleanup must not fabricate cancellation.
// Owner: durable workflow agent timeout cleanup.
#[tokio::test]
async fn uncertain_timeout_cleanup_does_not_become_cancellation() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let run_id = run.manifest.run_id;
    assert!(matches!(
        runner(
            home.path(),
            workspace.path(),
            Arc::new(TimeoutCleanupExecutor)
        )
        .drive(run_id, RecoveryDecision::NormalResume, None)
        .await,
        Err(RuntimeError::NeedsRecovery { .. })
    ));
    let store = WorkflowStore::new(home.path()).unwrap();
    let uncertain = store.load_run(run_id).unwrap();
    assert_eq!(uncertain.state.state.lifecycle, RunLifecycle::NeedsRecovery);
    assert!(!uncertain.state.state.cancellation_requested);
    assert!(!store
        .read_events(run_id)
        .unwrap()
        .iter()
        .any(|record| matches!(record.event, WorkflowEvent::CancellationRequested { .. })));
    let resumed = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor))
        .drive(run_id, RecoveryDecision::ConfirmNoProcess, None)
        .await
        .unwrap();
    assert_eq!(
        resumed.state.state.outcome(),
        Some(WorkflowOutcome::Success)
    );
}

// Covers: real process cancellation retains both streams and typed result across resume.
// Owner: durable workflow command runtime. Frozen handle launch is Linux/Android-only.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_command_cancellation_loads_and_resumes_with_complete_result() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let ready = workspace.path().join("ready");
    let marker = workspace.path().join("first-attempt");
    let run = create_run_with_workflow(
        home.path(),
        workspace.path(),
        command_workflow(workspace.path(), &ready, &marker),
    );
    let command_executor: Arc<dyn WorkflowNodeExecutor<CommandInvocation>> =
        Arc::new(WorkflowCommandExecutor::new(
            rho_sdk::ProcessEnvironment::Empty,
            Arc::new(AllowCommandHosts),
        ));
    let active_runner = Arc::new(WorkflowRunner::new(
        home.path().to_owned(),
        workspace.path().to_owned(),
        RuntimeSecurity {
            project_trusted: true,
            permission_mode: crate::permission::PermissionMode::Auto,
        },
        Arc::new(SuccessfulExecutor),
        Arc::clone(&command_executor),
    ));
    let worker_runner = Arc::clone(&active_runner);
    let run_id = run.manifest.run_id;
    let worker = tokio::spawn(async move {
        worker_runner
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await
    });
    // Early exit must not leave a detached drive holding run/checkout locks.
    struct AbortOnDrop(Option<tokio::task::JoinHandle<Result<StoredRun, RuntimeError>>>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            if let Some(handle) = self.0.take() {
                handle.abort();
            }
        }
    }
    let mut worker = AbortOnDrop(Some(worker));
    within_budget("command ready signal", async {
        loop {
            if tokio::fs::try_exists(&ready).await.unwrap_or(false) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    active_runner
        .cancellation_request(run_id)
        .request()
        .unwrap();
    let cancelled = join_within_budget(
        "real command cancellation",
        worker.0.take().expect("worker handle"),
    )
    .await
    .unwrap();
    let loaded = WorkflowStore::new(home.path())
        .unwrap()
        .load_run(run_id)
        .unwrap();
    assert_eq!(loaded.state, cancelled.state);
    let completion = completion(&loaded, "inspect");
    assert_eq!(completion.command_exit, Some(CommandExit::Cancellation));
    assert_eq!(completion.outcome, NodeTerminalState::Cancellation);
    let stdout = completion.artifacts.stdout.as_ref().unwrap();
    let stderr = completion.artifacts.stderr.as_ref().unwrap();
    assert!(stdout.retained_bytes <= 4);
    assert!(stderr.retained_bytes <= 4);
    let directory = WorkflowLayout::new(home.path()).run(run_id);
    let command_outcome: CommandOutcome = serde_json::from_slice(
        &std::fs::read(
            directory.join(
                &completion
                    .artifacts
                    .command_outcome
                    .as_ref()
                    .unwrap()
                    .relative_path,
            ),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(command_outcome.exit, CommandExit::Cancellation);
    assert_eq!(command_outcome.stdout, *stdout);
    assert_eq!(command_outcome.stderr, *stderr);
    let resumed_runner = WorkflowRunner::new(
        home.path().to_owned(),
        workspace.path().to_owned(),
        RuntimeSecurity {
            project_trusted: true,
            permission_mode: crate::permission::PermissionMode::Auto,
        },
        Arc::new(SuccessfulExecutor),
        command_executor,
    );
    let resumed = within_budget(
        "command resume",
        resumed_runner.drive(run_id, RecoveryDecision::NormalResume, None),
    )
    .await
    .unwrap();
    assert_eq!(
        resumed.state.state.outcome(),
        Some(WorkflowOutcome::Success)
    );
    assert_eq!(
        resumed.state.state.root_scope().command_exits[&node_id("inspect")],
        CommandExit::Code { code: 0 }
    );
}

// Covers: attacker-controlled checkout lock symlinks must not be opened/replaced.
// Owner: checkout gate filesystem lock boundary.
#[cfg(unix)]
#[test]
fn checkout_gate_rejects_symlink_lock() {
    use sha2::{Digest as _, Sha256};
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let locks = home.path().join("workflows/checkout-locks");
    std::fs::create_dir_all(&locks).unwrap();
    let canonical = workspace.path().canonicalize().unwrap();
    let key = format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().as_bytes())
    );
    let target = home.path().join("attacker-file");
    std::fs::write(&target, "do not open").unwrap();
    symlink(&target, locks.join(format!("{key}.lock"))).unwrap();
    assert!(CheckoutGate::new(home.path(), workspace.path()).is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "do not open");
}

// Covers: cancellation while another owner holds the checkout lock must not park exit.
// Owner: checkout gate cross-process wait.
#[tokio::test(flavor = "current_thread")]
async fn checkout_gate_lock_wait_honors_cancellation() {
    use fs2::FileExt;
    use sha2::{Digest as _, Sha256};
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let gate = CheckoutGate::new(home.path(), workspace.path()).unwrap();
    let canonical = workspace.path().canonicalize().unwrap();
    let key = format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().as_bytes())
    );
    let contender = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(
            home.path()
                .join("workflows/checkout-locks")
                .join(format!("{key}.lock")),
        )
        .unwrap();
    FileExt::lock_exclusive(&contender).unwrap();
    let cancellation = rho_sdk::CancellationToken::new();
    let wait_limit_seconds =
        test_workflow().program.root.nodes[&node_id("inspect")].timeout_seconds;
    // flock contends across same-process descriptors; Windows locks are process-scoped.
    #[cfg(unix)]
    let result = {
        let wait_entered = CheckoutGate::arm_lock_wait_signal();
        let acquire = tokio::spawn({
            let gate = gate.clone();
            let cancellation = cancellation.clone();
            async move {
                gate.acquire(WorkspaceAccess::Mutating, &cancellation, wait_limit_seconds)
                    .await
            }
        });
        within_budget("checkout gate entered lock wait", wait_entered)
            .await
            .expect("acquire enters wait while contender holds lock");
        cancellation.cancel();
        join_within_budget("checkout gate cancellation", acquire).await
    };
    #[cfg(windows)]
    let result = {
        cancellation.cancel();
        within_budget(
            "checkout gate cancellation",
            gate.acquire(WorkspaceAccess::Mutating, &cancellation, wait_limit_seconds),
        )
        .await
    };
    assert!(matches!(result, Err(RuntimeError::Cancelled)));
    FileExt::unlock(&contender).unwrap();
}
