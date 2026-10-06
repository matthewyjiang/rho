use std::path::Path;

use rho_sdk::SessionSnapshot;
use serde::Serialize;

use crate::workflow::{
    apply_durable_event, derive_snapshot, AgentRuntime, AttemptCheckpoint, AttemptNumber,
    AttemptState, ExternalOwner, FrozenWorkflow, LeafExecution, NodeResetReason, NodeState,
    NodeTerminalState, RunId, RunLifecycle, RunStateRecord, StoredRun, TaskInstanceId,
    WorkflowEvent, WorkflowEventRecord, WorkflowState, WorkflowStore,
};

use super::{
    cancellation::{read_cancellation_request, run_directory},
    journal::{completed_attempt, read_attempt_record, write_attempt, RunJournal},
    runner::send_event,
    RecoveryDecision, RuntimeError, RuntimeEvent,
};

/// Compute resume policy once, after replay and recovery of completed attempts.
pub(super) enum ResumePlan {
    Finished,
    FinishClosedScope,
    Resume {
        reopen_root: bool,
        cancelled: Vec<TaskInstanceId>,
        uncertain: Vec<TaskInstanceId>,
        needs_confirmation: bool,
    },
}

impl ResumePlan {
    pub(super) fn for_run(record: &RunStateRecord) -> Self {
        let state = &record.state;
        let cancelled = cancelled_nodes(record);
        let reopen_root = state
            .run_result()
            .is_some_and(|result| result.outcome == crate::workflow::WorkflowOutcome::Cancellation);
        if state.run_result().is_some() && !reopen_root {
            return if state.lifecycle == RunLifecycle::Completed && !state.cancellation_requested {
                Self::Finished
            } else {
                Self::FinishClosedScope
            };
        }
        let uncertain = uncertain_nodes(record);
        let needs_confirmation =
            !uncertain.is_empty() || state.lifecycle == RunLifecycle::NeedsRecovery;
        Self::Resume {
            reopen_root,
            cancelled,
            uncertain,
            needs_confirmation,
        }
    }
}

fn cancelled_nodes(record: &RunStateRecord) -> Vec<TaskInstanceId> {
    record
        .state
        .tasks()
        .filter_map(|(node, state)| {
            matches!(
                state,
                NodeState::Terminal {
                    outcome: NodeTerminalState::Cancellation
                }
            )
            .then_some(node)
        })
        .collect()
}

fn uncertain_nodes(state: &RunStateRecord) -> Vec<TaskInstanceId> {
    state
        .state
        .tasks()
        .filter_map(|(node, value)| matches!(value, NodeState::Running { .. }).then_some(node))
        .collect()
}

fn mark_uncertain_attempts(
    run_directory: &std::path::Path,
    state: &RunStateRecord,
) -> Result<(), RuntimeError> {
    for (node, node_state) in state.state.tasks() {
        if let NodeState::Running { attempt } = node_state {
            mark_attempt_uncertain(run_directory, &node, *attempt)?;
        }
    }
    Ok(())
}

pub(super) fn mark_attempt_uncertain(
    run_directory: &std::path::Path,
    node: &TaskInstanceId,
    attempt: AttemptNumber,
) -> Result<(), RuntimeError> {
    let record = read_attempt_record(run_directory, node, attempt)?;
    let owner = match record.state {
        AttemptState::Started { owner } | AttemptState::InterruptedUncertain { owner } => owner,
        AttemptState::LaunchIntended => ExternalOwner::Process { pid: 0 },
        AttemptState::Completed { .. } | AttemptState::CleanlyCancelled => {
            return Err(RuntimeError::Data(format!(
                "node '{node}' cleanup became uncertain after its attempt was terminal"
            )))
        }
    };
    write_attempt(
        run_directory,
        node,
        attempt,
        AttemptState::InterruptedUncertain { owner },
    )
}

/// What resume does with one attempt that an earlier process left running.
#[derive(Debug, Serialize)]
pub(crate) struct UncertainAttempt {
    pub(crate) node: TaskInstanceId,
    pub(crate) attempt: AttemptNumber,
    #[serde(flatten)]
    pub(crate) recovery: AttemptRecovery,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum AttemptRecovery {
    /// Continue the same attempt from its saved agent session.
    Continue {
        #[serde(skip)]
        snapshot: Box<SessionSnapshot>,
    },
    /// Discard the attempt and start a new one.
    Reset { reason: ResetReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResetReason {
    /// Cancellation was requested before the exit, so the attempt does not resume.
    CancellationRequested,
    /// Commands have no saved progress to continue from.
    CommandNode,
    /// Claude and Cursor agents do not save Rho checkpoints.
    ForeignAgent,
    /// The agent exited before its first checkpoint.
    NoCheckpoint,
    UnreadableCheckpoint,
}

impl UncertainAttempt {
    /// Decides from durable inputs only, so the dry run and resume agree.
    fn plan(
        run_directory: &Path,
        graph: &FrozenWorkflow,
        state: &WorkflowState,
        node: TaskInstanceId,
    ) -> Result<Self, RuntimeError> {
        let Some(NodeState::Running { attempt }) = state.task(&node).cloned() else {
            return Err(RuntimeError::Data(format!(
                "uncertain node '{node}' has no running attempt"
            )));
        };
        let leaf = graph
            .leaf(&node)
            .ok_or_else(|| RuntimeError::LaunchMetadata { node: node.clone() })?;
        let reset = |reason| AttemptRecovery::Reset { reason };
        let recovery = if state.cancellation_requested {
            reset(ResetReason::CancellationRequested)
        } else {
            match leaf.execution {
                LeafExecution::Command { .. } => reset(ResetReason::CommandNode),
                LeafExecution::Agent { resolved, .. } => match resolved.runtime {
                    AgentRuntime::ClaudeCli | AgentRuntime::Cursor | AgentRuntime::Antigravity => {
                        reset(ResetReason::ForeignAgent)
                    }
                    AgentRuntime::Rho => {
                        match AttemptCheckpoint::new(run_directory, &node, attempt).read() {
                            Ok(Some(snapshot)) => AttemptRecovery::Continue {
                                snapshot: Box::new(snapshot),
                            },
                            Ok(None) => reset(ResetReason::NoCheckpoint),
                            Err(error) => {
                                tracing::warn!(%error, %node, "could not read workflow agent checkpoint");
                                reset(ResetReason::UnreadableCheckpoint)
                            }
                        }
                    }
                },
            }
        };
        Ok(Self {
            node,
            attempt,
            recovery,
        })
    }
}

/// An uncertain attempt that resumes from its checkpoint in this process.
pub(super) struct ContinuedAttempt {
    pub(super) node: TaskInstanceId,
    pub(super) attempt: AttemptNumber,
    pub(super) snapshot: SessionSnapshot,
}

/// Drops the saved agent session of an attempt that can no longer continue.
/// A leftover file only costs disk space, so a failure is logged, not fatal.
pub(super) fn discard_checkpoint(
    run_directory: &Path,
    node: &TaskInstanceId,
    attempt: AttemptNumber,
) {
    if let Err(error) = AttemptCheckpoint::new(run_directory, node, attempt).discard() {
        tracing::warn!(%error, %node, "could not remove workflow agent checkpoint");
    }
}

/// Execute the precomputed policy. Confirmation precedes resets and acknowledgement:
/// no interrupted external process is ever silently assumed to have exited.
pub(super) fn recover_state(
    journal: &mut RunJournal,
    plan: ResumePlan,
    decision: RecoveryDecision,
    known_cancellation: Option<&str>,
    pending_cancellation: Option<String>,
    events: &Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
) -> Result<Vec<ContinuedAttempt>, RuntimeError> {
    if let ResumePlan::Resume {
        uncertain,
        needs_confirmation,
        ..
    } = &plan
    {
        if !uncertain.is_empty() {
            mark_uncertain_attempts(&journal.directory, &journal.run.state)?;
            journal.commit(WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::NeedsRecovery,
            })?;
            send_event(
                events,
                RuntimeEvent::NeedsRecovery {
                    nodes: uncertain.clone(),
                },
            );
        }
        if *needs_confirmation && decision != RecoveryDecision::ConfirmNoProcess {
            return Err(RuntimeError::NeedsRecovery {
                nodes: uncertain
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    }
    if journal.run.state.state.cancellation_requested {
        if let Some(request_id) = pending_cancellation {
            journal.commit(WorkflowEvent::CancellationAcknowledged { request_id })?;
        }
    }
    let mut continued = Vec::new();
    match plan {
        ResumePlan::Finished => return Ok(continued),
        ResumePlan::FinishClosedScope => {}
        ResumePlan::Resume {
            reopen_root,
            cancelled,
            uncertain,
            ..
        } => {
            if reopen_root {
                journal.commit(WorkflowEvent::ScopeReopened {
                    scope: crate::workflow::ScopeInstanceId::ROOT,
                })?;
            }
            if journal.run.state.state.lifecycle != RunLifecycle::Running {
                journal.commit(WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::Running,
                })?;
            }
            for node in uncertain {
                let UncertainAttempt {
                    node,
                    attempt,
                    recovery,
                } = UncertainAttempt::plan(
                    &journal.directory,
                    &journal.run.graph,
                    &journal.run.state.state,
                    node,
                )?;
                match recovery {
                    AttemptRecovery::Continue { snapshot } => {
                        write_attempt(
                            &journal.directory,
                            &node,
                            attempt,
                            AttemptState::Started {
                                owner: ExternalOwner::Process {
                                    pid: std::process::id(),
                                },
                            },
                        )?;
                        continued.push(ContinuedAttempt {
                            node,
                            attempt,
                            snapshot: *snapshot,
                        });
                    }
                    AttemptRecovery::Reset { .. } => {
                        discard_checkpoint(&journal.directory, &node, attempt);
                        journal.commit(WorkflowEvent::NodeReset {
                            node,
                            reason: NodeResetReason::InterruptedRecovery,
                        })?;
                    }
                }
            }
            for node in cancelled {
                journal.commit(WorkflowEvent::NodeReset {
                    node,
                    reason: NodeResetReason::CleanCancellation,
                })?;
            }
        }
    }
    // Remove the acknowledged request before clearing durable cancellation. A
    // crash in between can repeat this cleanup; the opposite ordering can feed
    // a stale request back into the resumed driver. Never remove a newer receipt
    // installed after the old file was removed (including across a crash).
    let run_id = journal.run.manifest.run_id;
    if let Some(request_id) = known_cancellation {
        if read_cancellation_request(&journal.store, run_id)?.as_deref() == Some(request_id) {
            journal.store.clear_cancellation_request(run_id)?;
        }
    }
    if journal.run.state.state.cancellation_requested {
        journal.commit(WorkflowEvent::CancellationCleared)?;
    }
    Ok(continued)
}

pub(super) fn recover_completed_transitions(journal: &mut RunJournal) -> Result<(), RuntimeError> {
    for CompletedAttempt {
        node,
        attempt,
        events,
    } in completed_attempts(&journal.directory, &journal.run.state.state)?
    {
        for event in events {
            journal.commit(event)?;
        }
        discard_checkpoint(&journal.directory, &node, attempt);
    }
    Ok(())
}

/// A running attempt whose `status.json` already records completion, left by a
/// crash after the attempt finished but before the journal did.
struct CompletedAttempt {
    node: TaskInstanceId,
    attempt: AttemptNumber,
    /// Journal events that finish the node, in commit order.
    events: Vec<WorkflowEvent>,
}

fn completed_attempts(
    run_directory: &Path,
    state: &WorkflowState,
) -> Result<Vec<CompletedAttempt>, RuntimeError> {
    let mut completed = Vec::new();
    for (node, node_state) in state.tasks() {
        let NodeState::Running { attempt } = *node_state else {
            continue;
        };
        let Some(completion) = completed_attempt(run_directory, &node, attempt)? else {
            continue;
        };
        let mut events = Vec::new();
        if let Some(output) = completion.structured_output.clone() {
            if state.structured_output(&node, attempt).is_none() {
                events.push(WorkflowEvent::StructuredOutput {
                    node: node.clone(),
                    attempt,
                    output,
                });
            }
        }
        events.push(WorkflowEvent::NodeFinished {
            node: node.clone(),
            completion: Box::new(completion),
        });
        completed.push(CompletedAttempt {
            node,
            attempt,
            events,
        });
    }
    Ok(completed)
}

/// Read-only report of what `resume` would do with uncertain attempts.
#[derive(Debug, Serialize)]
pub(crate) struct RecoveryPreview {
    pub(crate) run_id: RunId,
    /// Resume refuses until the caller confirms that no prior process remains.
    pub(crate) needs_confirmation: bool,
    pub(crate) attempts: Vec<UncertainAttempt>,
}

/// Plans recovery from the same inputs as resume, without the run lock or writes.
pub(crate) fn preview_recovery(
    rho_home: &Path,
    run_id: RunId,
) -> Result<RecoveryPreview, RuntimeError> {
    let (mut run, records) = WorkflowStore::new(rho_home)?.load_run_with_events(run_id)?;
    let directory = run_directory(rho_home, run_id);
    replay_event_tail(&mut run, &records, &directory)?;
    for completed in completed_attempts(&directory, &run.state.state)? {
        for event in completed.events {
            run.state.state =
                apply_durable_event(&run.graph, &run.state.state, &event, &directory)?;
        }
    }
    let (needs_confirmation, uncertain) = match ResumePlan::for_run(&run.state) {
        ResumePlan::Resume {
            needs_confirmation,
            uncertain,
            ..
        } => (needs_confirmation, uncertain),
        ResumePlan::Finished | ResumePlan::FinishClosedScope => (false, Vec::new()),
    };
    let attempts = uncertain
        .into_iter()
        .map(|node| UncertainAttempt::plan(&directory, &run.graph, &run.state.state, node))
        .collect::<Result<_, _>>()?;
    Ok(RecoveryPreview {
        run_id,
        needs_confirmation,
        attempts,
    })
}

/// Brings the saved state up to the last journal event. The snapshot may lag
/// the journal; the domain reducer replays the tail. Returns whether it changed.
pub(super) fn replay_event_tail(
    run: &mut StoredRun,
    records: &[WorkflowEventRecord],
    run_directory: &Path,
) -> Result<bool, RuntimeError> {
    let tail = records.last().map_or(0, |record| record.sequence);
    if tail == run.state.last_event_sequence {
        return Ok(false);
    }
    run.state.state = derive_snapshot(&run.graph, records, tail, run_directory)?;
    run.state.last_event_sequence = tail;
    Ok(true)
}
