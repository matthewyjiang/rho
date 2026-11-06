use super::{test_support::*, *};
use crate::workflow::*;
use pretty_assertions::assert_eq;
use std::{
    io::Write as _,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionCrashPoint {
    TerminalAttempt,
    StructuredOutput,
    NodeFinished,
    SnapshotSaved,
    ScopeFinished,
    ScopeFinishedDuringCancellation,
}

// Covers: every durable completion boundary must recover without rerunning work.
// Owner: workflow crash recovery.
#[tokio::test]
async fn terminal_completion_recovers_at_each_crash_point() {
    for point in [
        CompletionCrashPoint::TerminalAttempt,
        CompletionCrashPoint::StructuredOutput,
        CompletionCrashPoint::NodeFinished,
        CompletionCrashPoint::SnapshotSaved,
        CompletionCrashPoint::ScopeFinished,
        CompletionCrashPoint::ScopeFinishedDuringCancellation,
    ] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mut run =
            create_run_with_workflow(home.path(), workspace.path(), structured_workflow());
        let store = WorkflowStore::new(home.path()).unwrap();
        let directory = WorkflowLayout::new(home.path()).run(run.manifest.run_id);
        let attempt = AttemptNumber::new(1).unwrap();
        let node = task_id("inspect");
        let mut guard = store.lock_run(run.manifest.run_id).unwrap();
        for event in [
            WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::Running,
            },
            WorkflowEvent::NodeReady { node: node.clone() },
        ] {
            append_fixture_event(&store, &mut guard, &directory, &mut run, event);
        }
        let attempt_directory = attempt_directory(&directory, &node, attempt);
        std::fs::create_dir_all(&attempt_directory).unwrap();
        let output_artifact =
            artifacts::write_artifact(&directory, &attempt_directory.join("output.json"), b"true")
                .unwrap();
        let output = ValidatedOutputRef {
            artifact: output_artifact.clone(),
            value: WorkflowValue::Bool(true),
        };
        let completion = NodeCompletion {
            attempt: Some(attempt),
            outcome: NodeTerminalState::Success,
            cancellation_resume: None,
            command_exit: None,
            structured_output: Some(output.clone()),
            artifacts: AttemptArtifacts {
                structured_output: Some(output_artifact),
                ..AttemptArtifacts::default()
            },
        };
        artifacts::write_json(
            &directory,
            &attempt_directory.join("status.json"),
            &AttemptRecord {
                schema_version: ATTEMPT_VERSION,
                attempt,
                state: AttemptState::Completed {
                    completion: Box::new(completion.clone()),
                },
            },
        )
        .unwrap();
        for event in [
            WorkflowEvent::LaunchIntended {
                node: node.clone(),
                attempt,
            },
            WorkflowEvent::AttemptStarted {
                node: node.clone(),
                attempt,
                owner: ExternalOwner::Process { pid: 42 },
            },
        ] {
            append_fixture_event(&store, &mut guard, &directory, &mut run, event);
        }
        let structured = WorkflowEvent::StructuredOutput {
            node: node.clone(),
            attempt,
            output,
        };
        let finished = WorkflowEvent::NodeFinished {
            node,
            completion: Box::new(completion),
        };
        match point {
            CompletionCrashPoint::TerminalAttempt => {}
            CompletionCrashPoint::StructuredOutput | CompletionCrashPoint::NodeFinished => {
                let events = if point == CompletionCrashPoint::StructuredOutput {
                    vec![structured]
                } else {
                    vec![structured, finished]
                };
                for (offset, event) in events.into_iter().enumerate() {
                    store
                        .append_event(
                            &mut guard,
                            &WorkflowEventRecord {
                                schema_version: EVENT_VERSION,
                                sequence: run.state.last_event_sequence + offset as u64 + 1,
                                event,
                            },
                        )
                        .unwrap();
                }
            }
            CompletionCrashPoint::SnapshotSaved
            | CompletionCrashPoint::ScopeFinished
            | CompletionCrashPoint::ScopeFinishedDuringCancellation => {
                append_fixture_event(&store, &mut guard, &directory, &mut run, structured);
                append_fixture_event(&store, &mut guard, &directory, &mut run, finished);
                if point == CompletionCrashPoint::ScopeFinishedDuringCancellation {
                    append_fixture_event(
                        &store,
                        &mut guard,
                        &directory,
                        &mut run,
                        WorkflowEvent::CancellationRequested {
                            request_id: "00000000-0000-0000-0000-000000000007".into(),
                        },
                    );
                }
                if matches!(
                    point,
                    CompletionCrashPoint::ScopeFinished
                        | CompletionCrashPoint::ScopeFinishedDuringCancellation
                ) {
                    let scope = ScopeInstanceId::ROOT;
                    let result = scope_result(&run.graph, &run.state.state, scope)
                        .unwrap()
                        .unwrap();
                    append_fixture_event(
                        &store,
                        &mut guard,
                        &directory,
                        &mut run,
                        WorkflowEvent::ScopeFinished { scope, result },
                    );
                }
            }
        }
        drop(guard);
        let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
        let completed = runner(home.path(), workspace.path(), executor.clone())
            .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
            .await
            .unwrap();
        assert_eq!(executor.0.load(Ordering::SeqCst), 0, "point: {point:?}");
        assert_eq!(
            terminal(&completed, "inspect"),
            Some(NodeTerminalState::Success),
            "point: {point:?}"
        );
        assert_eq!(
            root_output(&completed, "inspect"),
            &WorkflowValue::Bool(true),
            "point: {point:?}"
        );
        assert_eq!(
            completed.state.state.outcome(),
            Some(WorkflowOutcome::Success),
            "point: {point:?}"
        );
        let artifact = super::test_support::completion(&completed, "inspect")
            .artifacts
            .structured_output
            .as_ref()
            .unwrap();
        assert_eq!(artifact.retained_bytes, 4, "point: {point:?}");
        assert_eq!(
            artifact.observed,
            ArtifactObservation::Complete { observed_bytes: 4 },
            "point: {point:?}"
        );
        assert_replay(home.path(), &completed);
        if point == CompletionCrashPoint::SnapshotSaved {
            std::fs::write(directory.join(&artifact.relative_path), b"false").unwrap();
            assert!(matches!(
                store.load_run(completed.manifest.run_id),
                Err(WorkflowError::Corrupt { .. })
            ));
        }
    }
}

// Covers: append-only launch intents, partial attempt setup, and a started event
// without its snapshot must not lose attempt numbering or bypass confirmation.
// Owner: workflow launch crash recovery.
#[tokio::test]
async fn launch_recovers_at_each_durable_boundary() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum Point {
        Intent,
        IntentFile,
        StartedFile,
        StartedEvent,
        StartedSnapshot,
    }
    for point in [
        Point::Intent,
        Point::IntentFile,
        Point::StartedFile,
        Point::StartedEvent,
        Point::StartedSnapshot,
    ] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let run = create_run(home.path(), workspace.path());
        let run_id = run.manifest.run_id;
        let store = WorkflowStore::new(home.path()).unwrap();
        let guard = store.lock_run(run_id).unwrap();
        let directory = WorkflowLayout::new(home.path()).run(run_id);
        let mut journal = journal::RunJournal {
            store,
            guard,
            directory,
            run,
        };
        let node = task_id("inspect");
        let attempt = AttemptNumber::new(1).unwrap();
        journal
            .commit(WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::Running,
            })
            .unwrap();
        journal
            .commit(WorkflowEvent::NodeReady { node: node.clone() })
            .unwrap();
        let before_intent = journal.run.state.clone();
        journal
            .commit(WorkflowEvent::LaunchIntended {
                node: node.clone(),
                attempt,
            })
            .unwrap();
        assert_eq!(journal.store.load_run(run_id).unwrap().state, before_intent);
        let attempt_directory = attempt_directory(&journal.directory, &node, attempt);
        if point >= Point::IntentFile {
            std::fs::create_dir_all(&attempt_directory).unwrap();
            artifacts::write_json(
                &journal.directory,
                &attempt_directory.join("status.json"),
                &AttemptRecord {
                    schema_version: ATTEMPT_VERSION,
                    attempt,
                    state: if point >= Point::StartedFile {
                        AttemptState::Started {
                            owner: ExternalOwner::Process { pid: 42 },
                            agent_session: None,
                        }
                    } else {
                        AttemptState::LaunchIntended
                    },
                },
            )
            .unwrap();
        }
        let started = WorkflowEvent::AttemptStarted {
            node,
            attempt,
            owner: ExternalOwner::Process { pid: 42 },
        };
        if point == Point::StartedEvent {
            journal
                .store
                .append_event(
                    &mut journal.guard,
                    &WorkflowEventRecord {
                        schema_version: EVENT_VERSION,
                        sequence: journal.run.state.last_event_sequence + 1,
                        event: started,
                    },
                )
                .unwrap();
        } else if point == Point::StartedSnapshot {
            journal.commit(started).unwrap();
        }
        drop(journal);
        let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
        let normal = runner(home.path(), workspace.path(), executor.clone())
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await;
        let completed = if point >= Point::StartedEvent {
            assert!(
                matches!(normal, Err(RuntimeError::NeedsRecovery { .. })),
                "{point:?}"
            );
            assert_eq!(executor.0.load(Ordering::SeqCst), 0);
            runner(home.path(), workspace.path(), executor.clone())
                .drive(run_id, RecoveryDecision::ConfirmNoProcess, None)
                .await
                .unwrap()
        } else {
            normal.unwrap()
        };
        assert_eq!(executor.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            completion(&completed, "inspect").attempt,
            Some(AttemptNumber::new(2).unwrap())
        );
        assert_replay(home.path(), &completed);
    }
}

// Covers: resume interrupted between cancellation cleanup writes must neither
// resurrect an acknowledged request nor erase a newer request after a crash.
// Owner: workflow cancellation recovery.
#[tokio::test]
async fn cancelled_resume_recovers_at_each_cleanup_boundary() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum Point {
        Reopened,
        Running,
        Reset,
        RequestRemoved,
        StateCleared,
        LegacyStateCleared,
        NewRequest,
    }
    for point in [
        Point::Reopened,
        Point::Running,
        Point::Reset,
        Point::RequestRemoved,
        Point::StateCleared,
        Point::LegacyStateCleared,
        Point::NewRequest,
    ] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let run = create_run(home.path(), workspace.path());
        let run_id = run.manifest.run_id;
        let cancelling = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor));
        let receipt = cancelling.cancellation_request(run_id).request().unwrap();
        let cancelled = cancelling
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await
            .unwrap();
        assert_eq!(
            terminal(&cancelled, "inspect"),
            Some(NodeTerminalState::Cancellation)
        );
        assert!(cancellation_request_acknowledged(home.path(), run_id, &receipt).unwrap());
        let store = WorkflowStore::new(home.path()).unwrap();
        let guard = store.lock_run(run_id).unwrap();
        let directory = WorkflowLayout::new(home.path()).run(run_id);
        let mut journal = journal::RunJournal {
            store,
            guard,
            directory,
            run: cancelled,
        };
        journal
            .commit(WorkflowEvent::ScopeReopened {
                scope: ScopeInstanceId::ROOT,
            })
            .unwrap();
        if point >= Point::Running {
            journal
                .commit(WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::Running,
                })
                .unwrap();
        }
        if point >= Point::Reset {
            journal
                .commit(WorkflowEvent::NodeReset {
                    node: task_id("inspect"),
                    reason: NodeResetReason::CleanCancellation,
                })
                .unwrap();
        }
        if point >= Point::RequestRemoved && point != Point::LegacyStateCleared {
            journal.store.clear_cancellation_request(run_id).unwrap();
        }
        if matches!(point, Point::StateCleared | Point::LegacyStateCleared) {
            journal.commit(WorkflowEvent::CancellationCleared).unwrap();
        }
        let newer = (point == Point::NewRequest)
            .then(|| request_cross_process_cancel(home.path(), run_id).unwrap());
        drop(journal);
        let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
        let resumed = runner(home.path(), workspace.path(), executor.clone())
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await
            .unwrap();
        if let Some(newer) = newer {
            assert_ne!(newer, receipt);
            assert_eq!(
                terminal(&resumed, "inspect"),
                Some(NodeTerminalState::Cancellation)
            );
            assert!(cancellation_request_acknowledged(home.path(), run_id, &newer).unwrap());
            assert_eq!(executor.0.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(
                terminal(&resumed, "inspect"),
                Some(NodeTerminalState::Success),
                "{point:?}"
            );
            assert_eq!(executor.0.load(Ordering::SeqCst), 1);
        }
        assert_replay(home.path(), &resumed);
    }
}

// Covers: a flushed tail and a torn final line must replay before scheduling.
// Owner: workflow journal replay.
#[tokio::test]
async fn runner_replays_journal_tail() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let store = WorkflowStore::new(home.path()).unwrap();
    let mut guard = store.lock_run(run.manifest.run_id).unwrap();
    for (index, event) in [
        WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Running,
        },
        WorkflowEvent::NodeReady {
            node: task_id("inspect"),
        },
    ]
    .into_iter()
    .enumerate()
    {
        store
            .append_event(
                &mut guard,
                &WorkflowEventRecord {
                    schema_version: EVENT_VERSION,
                    sequence: index as u64 + 1,
                    event,
                },
            )
            .unwrap();
    }
    drop(guard);
    std::fs::OpenOptions::new()
        .append(true)
        .open(WorkflowLayout::new(home.path()).run_events(run.manifest.run_id))
        .unwrap()
        .write_all(b"{\"schema_version\":")
        .unwrap();
    let completed = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor))
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();
    assert_eq!(completed.state.state.lifecycle, RunLifecycle::Completed);
    assert_eq!(
        terminal(&completed, "inspect"),
        Some(NodeTerminalState::Success)
    );
    assert!(completed.state.last_event_sequence > 1);
    let records = store.read_events(run.manifest.run_id).unwrap();
    assert_eq!(records.last().unwrap().sequence, records.len() as u64);
}

// Covers: a prior running attempt requires explicit recovery and retains its
// owner and the session its agent runs in. Owner: workflow crash recovery.
#[tokio::test]
async fn uncertain_attempt_requires_explicit_recovery() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = rho_sdk::SessionId::new();
    let (run, directory) = seed_running_attempt(home.path(), workspace.path(), Some(&session));
    let error = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor))
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap_err();
    assert!(matches!(error, RuntimeError::NeedsRecovery { .. }));
    let store = WorkflowStore::new(home.path()).unwrap();
    assert_eq!(
        store
            .load_run(run.manifest.run_id)
            .unwrap()
            .state
            .state
            .lifecycle,
        RunLifecycle::NeedsRecovery
    );
    assert_eq!(
        read_attempt_state(&directory, 1),
        AttemptState::InterruptedUncertain {
            owner: ExternalOwner::Process { pid: 4242 },
            agent_session: Some(session),
        }
    );
}

/// Seeds attempt 1 of `inspect` as started by a process that is gone, its
/// agent running in `agent_session`, or in none an older Rho recorded.
fn seed_running_attempt(
    home: &std::path::Path,
    workspace: &std::path::Path,
    agent_session: Option<&rho_sdk::SessionId>,
) -> (StoredRun, std::path::PathBuf) {
    let mut run = create_run(home, workspace);
    let node = task_id("inspect");
    let attempt = AttemptNumber::new(1).unwrap();
    let store = WorkflowStore::new(home).unwrap();
    let directory = WorkflowLayout::new(home).run(run.manifest.run_id);
    let mut guard = store.lock_run(run.manifest.run_id).unwrap();
    for event in [
        WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Running,
        },
        WorkflowEvent::NodeReady { node: node.clone() },
        WorkflowEvent::LaunchIntended {
            node: node.clone(),
            attempt,
        },
        WorkflowEvent::AttemptStarted {
            node: node.clone(),
            attempt,
            owner: ExternalOwner::Process { pid: 4242 },
        },
    ] {
        append_fixture_event(&store, &mut guard, &directory, &mut run, event);
    }
    drop(guard);
    let attempt_directory = attempt_directory(&directory, &node, attempt);
    ensure_directory_beneath(
        &directory,
        attempt_directory.strip_prefix(&directory).unwrap(),
    )
    .unwrap();
    artifacts::write_json(
        &directory,
        &attempt_directory.join("status.json"),
        &AttemptRecord {
            schema_version: ATTEMPT_VERSION,
            attempt,
            state: AttemptState::Started {
                owner: ExternalOwner::Process { pid: 4242 },
                agent_session: agent_session.cloned(),
            },
        },
    )
    .unwrap();
    (run, directory)
}

/// The state `status.json` records for attempt `attempt` of `inspect`.
fn read_attempt_state(directory: &std::path::Path, attempt: u32) -> AttemptState {
    journal::read_attempt_record(
        directory,
        &task_id("inspect"),
        AttemptNumber::new(attempt).unwrap(),
    )
    .unwrap()
    .state
}

/// One dispatch an executor received: its attempt, the session it was handed,
/// and the session its `status.json` recorded by then.
#[derive(Debug, PartialEq)]
struct Dispatch {
    attempt: u32,
    session: Option<crate::app::headless_host::ChildSession>,
    recorded: Option<rho_sdk::SessionId>,
}

/// Records every dispatch, then succeeds.
#[derive(Default)]
struct SessionProbe {
    seen: std::sync::Mutex<Vec<Dispatch>>,
}

impl<I: Send + 'static> WorkflowNodeExecutor<I> for SessionProbe {
    fn execute<'a>(&'a self, request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a> {
        let recorded = match read_attempt_state(&request.run_directory, request.attempt.get()) {
            AttemptState::Started { agent_session, .. } => agent_session,
            state => panic!("dispatched attempt is {state:?}, not started"),
        };
        self.seen.lock().unwrap().push(Dispatch {
            attempt: request.attempt.get(),
            session: request.agent_session.clone(),
            recorded,
        });
        Box::pin(async { Ok(NodeExecutionResult::terminal(NodeTerminalState::Success)) })
    }
}

fn preview_json(home: &std::path::Path, run: &StoredRun) -> serde_json::Value {
    serde_json::to_value(preview_recovery(home, run.manifest.run_id).unwrap()).unwrap()
}

// Covers: a Rho attempt that recorded its session continues as the same
// attempt by reopening that session, and the dry run predicts that without
// writing. Owner: workflow crash recovery.
#[tokio::test]
async fn recorded_agent_session_continues_the_attempt() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = rho_sdk::SessionId::new();
    let (run, _) = seed_running_attempt(home.path(), workspace.path(), Some(&session));
    let events = WorkflowLayout::new(home.path()).run_events(run.manifest.run_id);
    let events_before = std::fs::read(&events).unwrap();

    assert_eq!(
        preview_json(home.path(), &run),
        serde_json::json!({
            "run_id": run.manifest.run_id,
            "needs_confirmation": true,
            "attempts": [{"node": "s0.inspect", "attempt": 1, "action": "continue"}],
        })
    );
    assert_eq!(std::fs::read(&events).unwrap(), events_before);

    let probe = Arc::new(SessionProbe::default());
    let completed = runner(home.path(), workspace.path(), probe.clone())
        .drive(
            run.manifest.run_id,
            RecoveryDecision::ConfirmNoProcess,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        *probe.seen.lock().unwrap(),
        [Dispatch {
            attempt: 1,
            session: Some(crate::app::headless_host::ChildSession::Reopen(
                session.clone()
            )),
            recorded: Some(session),
        }]
    );
    assert_eq!(
        terminal(&completed, "inspect"),
        Some(NodeTerminalState::Success)
    );
}

// Covers: uncertain Rho attempts that cannot continue restart as a new attempt
// in a new session, recorded before dispatch, and the dry run reports the same
// reason resume acts on. Owner: workflow crash recovery.
#[tokio::test]
async fn uncertain_agent_attempt_resets_when_it_cannot_continue() {
    for (recorded, cancel, reason) in [
        (false, false, "no_session_recorded"),
        (true, true, "cancellation_requested"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let old_session = rho_sdk::SessionId::new();
        let (mut run, directory) = seed_running_attempt(
            home.path(),
            workspace.path(),
            recorded.then_some(&old_session),
        );
        if cancel {
            let store = WorkflowStore::new(home.path()).unwrap();
            let mut guard = store.lock_run(run.manifest.run_id).unwrap();
            append_fixture_event(
                &store,
                &mut guard,
                &directory,
                &mut run,
                WorkflowEvent::CancellationRequested {
                    request_id: uuid::Uuid::new_v4().to_string(),
                },
            );
        }

        assert_eq!(
            preview_json(home.path(), &run)["attempts"],
            serde_json::json!([
                {"node": "s0.inspect", "attempt": 1, "action": "reset", "reason": reason},
            ]),
            "{reason}"
        );
        let probe = Arc::new(SessionProbe::default());
        runner(home.path(), workspace.path(), probe.clone())
            .drive(
                run.manifest.run_id,
                RecoveryDecision::ConfirmNoProcess,
                None,
            )
            .await
            .unwrap();
        let seen = probe.seen.lock().unwrap();
        let Some(crate::app::headless_host::ChildSession::New(new_session)) =
            seen.first().and_then(|dispatch| dispatch.session.clone())
        else {
            panic!("{reason}: the new attempt has no new session: {seen:?}");
        };
        assert_ne!(new_session, old_session, "{reason}");
        assert_eq!(
            *seen,
            [Dispatch {
                attempt: 2,
                session: Some(crate::app::headless_host::ChildSession::New(
                    new_session.clone()
                )),
                recorded: Some(new_session),
            }],
            "{reason}"
        );
    }
}
