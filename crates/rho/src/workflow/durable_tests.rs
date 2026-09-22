use pretty_assertions::assert_eq;

use super::*;
use crate::workflow::{
    test_support::{agent_node, id, task_id, workflow},
    ArtifactObservation, ArtifactRef, Digest, ExternalOwner, NodeExecution, OutputSchema,
    RunLifecycle, WorkflowEventRecord, WorkflowValue, WorkspaceAccess, EVENT_VERSION,
};

fn journal() -> (FrozenWorkflow, Vec<WorkflowEventRecord>) {
    let mut node = agent_node("work", &[], WorkspaceAccess::ReadOnly);
    let NodeExecution::Agent(agent) = &mut node.execution else {
        unreachable!()
    };
    agent.output = Some(OutputSchema::Bool);
    let graph = workflow(vec![node]);
    let attempt = AttemptNumber::new(1).unwrap();
    let output = ValidatedOutputRef {
        artifact: ArtifactRef {
            relative_path: "output.json".into(),
            retained_bytes: 4,
            observed: ArtifactObservation::Complete { observed_bytes: 4 },
            digest: Digest("fixture".into()),
        },
        value: WorkflowValue::Bool(true),
    };
    let mut completion = NodeCompletion::terminal(NodeTerminalState::Success);
    completion.attempt = Some(attempt);
    completion.structured_output = Some(output.clone());
    let events = vec![
        WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Running,
        },
        WorkflowEvent::NodeReady {
            node: task_id("work"),
        },
        WorkflowEvent::LaunchIntended {
            node: task_id("work"),
            attempt,
        },
        WorkflowEvent::AttemptStarted {
            node: task_id("work"),
            attempt,
            owner: ExternalOwner::Process { pid: 1 },
        },
        WorkflowEvent::StructuredOutput {
            node: task_id("work"),
            attempt,
            output,
        },
        WorkflowEvent::NodeFinished {
            node: task_id("work"),
            completion: Box::new(completion),
        },
        WorkflowEvent::ScopeFinished {
            scope: crate::workflow::ScopeInstanceId::ROOT,
            result: crate::workflow::ScopeResult {
                outcome: crate::workflow::WorkflowOutcome::Success,
                outputs: BTreeMap::new(),
            },
        },
        WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Completed,
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(index, event)| WorkflowEventRecord {
        schema_version: EVENT_VERSION,
        sequence: index as u64 + 1,
        event,
    })
    .collect();
    (graph, events)
}

// Covers: a snapshot between output and completion must retain pairing evidence.
// Owner: durable workflow reduction, shared by full and incremental replay.
#[test]
fn every_snapshot_boundary_replays_identically() {
    let (graph, events) = journal();
    let path = Path::new("state.json");
    let expected = derive_snapshot(&graph, &events, events.len() as u64, path).unwrap();
    for boundary in 0..=events.len() {
        let snapshot = derive_snapshot(&graph, &events, boundary as u64, path).unwrap();
        let mut restored: WorkflowState =
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        for record in &events[boundary..] {
            restored = apply_durable_event(&graph, &restored, &record.event, path).unwrap();
        }
        assert_eq!(&restored, &expected, "snapshot boundary {boundary}");
    }
}

// Covers: forged or duplicate output/completion events must fail on live replay too.
#[test]
fn incremental_replay_rejects_invalid_attempt_pairs() {
    let (graph, events) = journal();
    let path = Path::new("state.json");
    let mut wrong_completion = events[5].event.clone();
    if let WorkflowEvent::NodeFinished { completion, .. } = &mut wrong_completion {
        completion.attempt = Some(AttemptNumber::new(2).unwrap());
    }
    let mut mismatched_output = events[5].event.clone();
    if let WorkflowEvent::NodeFinished { completion, .. } = &mut mismatched_output {
        completion.structured_output.as_mut().unwrap().value = WorkflowValue::Bool(false);
    }
    let mut synthetic_with_data = events[5].event.clone();
    if let WorkflowEvent::NodeFinished { completion, .. } = &mut synthetic_with_data {
        completion.attempt = None;
        completion.outcome = NodeTerminalState::Cancellation;
    }
    let mut missing_attempt = events[5].event.clone();
    if let WorkflowEvent::NodeFinished { completion, .. } = &mut missing_attempt {
        completion.attempt = None;
    }
    for (through, event) in [
        (2, events[3].event.clone()), // Start without allocation.
        (3, events[4].event.clone()), // Output before start.
        (4, events[5].event.clone()), // Completion before output record.
        (5, events[4].event.clone()), // Duplicate output.
        (5, wrong_completion),
        (5, missing_attempt),
        (5, mismatched_output),
        (2, synthetic_with_data),
        (0, WorkflowEvent::CancellationCleared),
        (
            5,
            WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::Completed,
            },
        ),
    ] {
        let state = derive_snapshot(&graph, &events, through, path).unwrap();
        assert!(
            apply_durable_event(&graph, &state, &event, path).is_err(),
            "through {through}: {event:?}"
        );
    }
}

// Covers: replay must reject work admitted after cancellation or during recovery.
// Owner: durable workflow admission, including persisted histories.
#[test]
fn work_admission_requires_an_active_uncancelled_run() {
    let (graph, events) = journal();
    let path = Path::new("state.json");
    for (through, event) in [
        (1, &events[1].event),
        (2, &events[2].event),
        (3, &events[3].event),
    ] {
        let base = derive_snapshot(&graph, &events, through, path).unwrap();
        for (lifecycle, cancellation_requested) in [
            (RunLifecycle::Planned, false),
            (RunLifecycle::NeedsRecovery, false),
            (RunLifecycle::Cancelling, true),
            (RunLifecycle::Running, true),
        ] {
            let state = WorkflowState {
                lifecycle,
                cancellation_requested,
                ..base.clone()
            };
            assert!(matches!(
                apply_durable_event(&graph, &state, event, path),
                Err(WorkflowError::Corrupt { .. })
            ));
        }
    }
}

// Covers: crashing after reserving an attempt cannot reuse its artifact directory.
#[test]
fn unstarted_reservation_is_consumed_and_can_be_superseded() {
    let (graph, events) = journal();
    let path = Path::new("state.json");
    let state = derive_snapshot(&graph, &events, 3, path).unwrap();
    let attempt = state.next_attempt(&task_id("work")).unwrap();
    assert_eq!(attempt, AttemptNumber::new(2).unwrap());
    let state = apply_durable_event(
        &graph,
        &state,
        &WorkflowEvent::LaunchIntended {
            node: task_id("work"),
            attempt,
        },
        path,
    )
    .unwrap();
    assert!(apply_durable_event(&graph, &state, &events[3].event, path).is_err());
    let started = apply_durable_event(
        &graph,
        &state,
        &WorkflowEvent::AttemptStarted {
            node: task_id("work"),
            attempt,
            owner: ExternalOwner::Process { pid: 1 },
        },
        path,
    )
    .unwrap();
    assert_eq!(
        started.task(&task_id("work")),
        Some(&NodeState::Running { attempt })
    );
    let reset = apply_durable_event(
        &graph,
        &started,
        &WorkflowEvent::NodeReset {
            node: task_id("work"),
            reason: crate::workflow::NodeResetReason::InterruptedRecovery,
        },
        path,
    )
    .unwrap();
    assert_eq!(
        reset.next_attempt(&task_id("work")).unwrap(),
        AttemptNumber::new(3).unwrap()
    );
}

// Covers: synthetic cancellation records the actual waiting state for resume.
#[test]
fn synthetic_cancellation_preserves_waiting_state() {
    let (graph, events) = journal();
    let path = Path::new("state.json");
    let WorkflowEvent::NodeFinished { completion, .. } = &events[5].event else {
        unreachable!()
    };
    let mut synthetic_with_output = completion.as_ref().clone();
    synthetic_with_output.attempt = None;
    synthetic_with_output.outcome = NodeTerminalState::Skipped;
    for (through, resume) in [
        (1, CancellationResumeState::Pending),
        (2, CancellationResumeState::Ready),
    ] {
        let state = derive_snapshot(&graph, &events, through, path).unwrap();
        let next = apply_durable_event(
            &graph,
            &state,
            &WorkflowEvent::NodeFinished {
                node: task_id("work"),
                completion: Box::new(NodeCompletion::cancelled(resume)),
            },
            path,
        )
        .unwrap();
        assert_eq!(
            next.root_scope().completions[&id("work")],
            NodeCompletion::cancelled(resume)
        );
        let other_resume = match resume {
            CancellationResumeState::Pending => CancellationResumeState::Ready,
            CancellationResumeState::Ready => CancellationResumeState::Pending,
        };
        for invalid in [
            NodeCompletion::cancelled(other_resume),
            NodeCompletion::terminal(NodeTerminalState::Cancellation),
            synthetic_with_output.clone(),
        ] {
            assert!(matches!(
                apply_durable_event(
                    &graph,
                    &state,
                    &WorkflowEvent::NodeFinished {
                        node: task_id("work"),
                        completion: Box::new(invalid),
                    },
                    path,
                ),
                Err(WorkflowError::Corrupt { .. })
            ));
        }
    }
}

// Covers: explicit scope closure is replay-validated and cancelled resume keeps
// successful siblings and attempt allocation. Owner: durable workflow reducer.
#[test]
fn scope_closure_and_cancelled_reopen_preserve_instance_history() {
    use crate::workflow::{NodeResetReason, ScopeInstanceId, WorkflowOutcome};
    let (mut graph, records) = journal();
    let sibling = agent_node("cancelled", &[], WorkspaceAccess::ReadOnly);
    graph.program.root.nodes.insert(sibling.id.clone(), sibling);
    let path = Path::new("state.json");
    let mut state = WorkflowState::new(&graph);
    for record in &records[..6] {
        state = apply_durable_event(&graph, &state, &record.event, path).unwrap();
    }
    let root = ScopeInstanceId::ROOT;
    assert!(apply_durable_event(&graph, &state, &records[6].event, path).is_err());
    for event in [
        WorkflowEvent::CancellationRequested {
            request_id: "cancel".into(),
        },
        WorkflowEvent::NodeFinished {
            node: task_id("cancelled"),
            completion: Box::new(NodeCompletion::cancelled(CancellationResumeState::Pending)),
        },
    ] {
        state = apply_durable_event(&graph, &state, &event, path).unwrap();
    }
    let result = crate::workflow::scope_result(&graph, &state, root)
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome, WorkflowOutcome::Cancellation);
    let mut forged = result.clone();
    forged.outcome = WorkflowOutcome::Success;
    assert!(apply_durable_event(
        &graph,
        &state,
        &WorkflowEvent::ScopeFinished {
            scope: root,
            result: forged
        },
        path
    )
    .is_err());
    state = apply_durable_event(
        &graph,
        &state,
        &WorkflowEvent::ScopeFinished {
            scope: root,
            result,
        },
        path,
    )
    .unwrap();
    state = apply_durable_event(
        &graph,
        &state,
        &WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Completed,
        },
        path,
    )
    .unwrap();
    let reset = WorkflowEvent::NodeReset {
        node: task_id("cancelled"),
        reason: NodeResetReason::CleanCancellation,
    };
    for event in [
        reset.clone(),
        WorkflowEvent::CancellationRequested {
            request_id: "again".into(),
        },
        WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Cancelling,
        },
    ] {
        assert!(apply_durable_event(&graph, &state, &event, path).is_err());
    }
    for lifecycle in [RunLifecycle::Planned, RunLifecycle::NeedsRecovery] {
        let invalid = WorkflowState {
            lifecycle,
            ..state.clone()
        };
        assert!(matches!(
            apply_durable_event(
                &graph,
                &invalid,
                &WorkflowEvent::ScopeReopened { scope: root },
                path,
            ),
            Err(WorkflowError::Corrupt { .. })
        ));
    }
    state = apply_durable_event(
        &graph,
        &state,
        &WorkflowEvent::ScopeReopened { scope: root },
        path,
    )
    .unwrap();
    assert_eq!(state.outcome(), None);
    state = apply_durable_event(&graph, &state, &reset, path).unwrap();
    assert_eq!(
        state.task(&task_id("work")),
        Some(&NodeState::Terminal {
            outcome: NodeTerminalState::Success
        })
    );
    assert_eq!(state.task(&task_id("cancelled")), Some(&NodeState::Pending));
    assert_eq!(
        state.next_attempt(&task_id("work")).unwrap(),
        AttemptNumber::new(2).unwrap()
    );
    assert!(apply_durable_event(
        &graph,
        &state,
        &WorkflowEvent::NodeReset {
            node: task_id("work"),
            reason: NodeResetReason::CleanCancellation
        },
        path
    )
    .is_err());
    let completed = derive_snapshot(&journal().0, &records, records.len() as u64, path).unwrap();
    assert!(apply_durable_event(
        &journal().0,
        &completed,
        &WorkflowEvent::ScopeReopened { scope: root },
        path
    )
    .is_err());
}
