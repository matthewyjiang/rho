use std::{future::Future, pin::Pin};

#[cfg(debug_assertions)]
use std::{collections::VecDeque, str::FromStr, time::Duration};

pub(crate) use crate::workflow::ArtifactKind;
#[cfg(debug_assertions)]
use crate::workflow::NodeTerminalState;
use crate::workflow::{
    AgentRuntime, ArtifactRef, AttemptNumber, CommandExit, Digest, ExternalOwner, NodeId,
    NodeState, PlanId, RunId, RunLifecycle, WorkflowOutcome, WorkflowValue, WorkspaceAccess,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanApprovalState {
    AwaitingPlan,
    AwaitingResume,
    Approved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CancellationState {
    NotRequested,
    Requested,
    Saved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceDigestSummary {
    pub(crate) source_count: usize,
    pub(crate) digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExecutionMetadata {
    Agent {
        name: String,
        runtime: AgentRuntime,
        provider: Option<String>,
        model: Option<String>,
    },
    Command {
        executable: String,
        cwd: String,
        shell: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactReference {
    pub(crate) kind: ArtifactKind,
    pub(crate) artifact: ArtifactRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalReason {
    Failure(String),
    Denial(String),
    Cancellation(String),
    Blocked(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecoveryRequirement {
    pub(crate) node: NodeId,
    pub(crate) attempt: AttemptNumber,
    pub(crate) uncertain_owner: ExternalOwner,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowProgress {
    pub(crate) attempt: AttemptNumber,
    pub(crate) completed: Option<u64>,
    pub(crate) total: Option<u64>,
    pub(crate) message: String,
    pub(crate) detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowNodeSnapshot {
    pub(crate) id: NodeId,
    pub(crate) display_name: String,
    pub(crate) dependencies: Vec<NodeId>,
    pub(crate) access: WorkspaceAccess,
    pub(crate) execution: ExecutionMetadata,
    /// Short task description from the plan (prompt/command), always available.
    pub(crate) work: String,
    pub(crate) state: NodeState,
    pub(crate) current_attempt: Option<AttemptNumber>,
    pub(crate) command_exit: Option<CommandExit>,
    pub(crate) validated_output: Option<WorkflowValue>,
    pub(crate) artifacts: Vec<ArtifactReference>,
    pub(crate) terminal_reason: Option<TerminalReason>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowSnapshot {
    pub(crate) workflow_name: String,
    pub(crate) plan_id: PlanId,
    pub(crate) run_id: Option<RunId>,
    pub(crate) graph_digest: Digest,
    pub(crate) sources: SourceDigestSummary,
    pub(crate) approval: PlanApprovalState,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) outcome: Option<WorkflowOutcome>,
    /// Nodes are already in frozen scheduler order. The TUI never re-sorts them.
    pub(crate) nodes: Vec<WorkflowNodeSnapshot>,
    pub(crate) cancellation: CancellationState,
    pub(crate) recovery_requirement: Option<RecoveryRequirement>,
    /// True only after an owner-mode runner has saved state and stopped active
    /// handles. Watch sessions ignore this for leave permission.
    pub(crate) exit_is_safe: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowEvent {
    Snapshot(WorkflowSnapshot),
    Progress {
        node: NodeId,
        progress: WorkflowProgress,
    },
    Notice(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowAction {
    ConfirmPlan,
    ConfirmResume,
    Cancel,
}

/// How this TUI session relates to the workflow driver.
///
/// Session mode is fixed for the adapter lifetime. It is not run state and must
/// not be stamped onto [`WorkflowSnapshot`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowSession {
    /// Owns the driver. Leave only after durable stop (`exit_is_safe`).
    Owner,
    /// Observes store snapshots only. Leave anytime; cancel still requests stop.
    Watcher,
}

/// Adapter contract between the workflow runner and this terminal mode.
///
/// `send` must persist or apply an action before it returns. `next_event` must
/// yield typed durable snapshots in order. In particular, it must not set
/// `exit_is_safe` until all active work has stopped and state has been saved.
///
/// [`WorkflowSession::Watcher`] adapters may no-op [`Self::shutdown`] and must
/// still report honest `exit_is_safe` values from the store.
pub(crate) trait WorkflowEventAdapter: Send {
    /// Session relationship for this adapter.
    ///
    /// Required with no default so a new adapter cannot silently become an
    /// owner UI (leave blocked, Esc cancels).
    fn session(&self) -> WorkflowSession;

    fn initial_snapshot(&self) -> WorkflowSnapshot;

    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<WorkflowEvent>>> + Send + '_>>;

    fn send(
        &mut self,
        action: WorkflowAction,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;

    /// Owner: stop active handles and save durable state before returning.
    /// Watcher: may return immediately; the background driver owns cleanup.
    ///
    /// The TUI calls this after any screen, input, or event-source error.
    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MatrixWorkflowStart {
    Run,
    Resume,
}

#[cfg(debug_assertions)]
pub(crate) const MATRIX_WORKFLOW_PLAN_ID: &str = "00000000-0000-4000-8000-000000000674";
#[cfg(debug_assertions)]
pub(crate) const MATRIX_WORKFLOW_RUN_ID: &str = "00000000-0000-4000-8000-000000000675";

/// Creates a deterministic workflow source for PTY tests.
///
/// The CLI integration should call this only when `RHO_TUI_TEST_MODE=matrix`.
/// This source does not use the provider fixture or automation tools.
#[cfg(debug_assertions)]
pub(crate) fn matrix_adapter(start: MatrixWorkflowStart) -> Box<dyn WorkflowEventAdapter> {
    Box::new(MatrixAdapter::new(start))
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatrixStage {
    AwaitingApproval,
    Parallel,
    Mutating,
    Cancelling,
    Complete,
}

#[cfg(debug_assertions)]
struct MatrixAdapter {
    start: MatrixWorkflowStart,
    snapshot: WorkflowSnapshot,
    queued: VecDeque<WorkflowEvent>,
    stage: MatrixStage,
}

#[cfg(debug_assertions)]
impl MatrixAdapter {
    fn new(start: MatrixWorkflowStart) -> Self {
        Self {
            start,
            snapshot: matrix_snapshot(start),
            queued: VecDeque::new(),
            stage: MatrixStage::AwaitingApproval,
        }
    }

    fn begin(&mut self) {
        self.snapshot.approval = PlanApprovalState::Approved;
        self.snapshot.lifecycle = RunLifecycle::Running;
        self.snapshot.exit_is_safe = false;
        for node in &mut self.snapshot.nodes {
            if node.id.as_str() == "inspect" && self.start == MatrixWorkflowStart::Resume {
                continue;
            }
            if node.id.as_str() == "inspect" || node.id.as_str() == "test" {
                let attempt = if self.start == MatrixWorkflowStart::Resume {
                    AttemptNumber::new(2).expect("valid matrix attempt")
                } else {
                    AttemptNumber::new(1).expect("valid matrix attempt")
                };
                node.state = NodeState::Running { attempt };
                node.current_attempt = Some(attempt);
            }
        }
        self.stage = MatrixStage::Parallel;
        self.queued
            .push_back(WorkflowEvent::Snapshot(self.snapshot.clone()));
    }

    fn request_cancel(&mut self) {
        if self.snapshot.exit_is_safe {
            return;
        }
        self.snapshot.lifecycle = RunLifecycle::Cancelling;
        self.snapshot.cancellation = CancellationState::Requested;
        self.stage = MatrixStage::Cancelling;
        self.queued
            .push_back(WorkflowEvent::Snapshot(self.snapshot.clone()));
    }

    fn advance(&mut self) -> Option<WorkflowEvent> {
        match self.stage {
            MatrixStage::AwaitingApproval | MatrixStage::Complete => None,
            MatrixStage::Parallel => {
                let test_attempt = self
                    .snapshot
                    .nodes
                    .iter()
                    .find(|node| node.id.as_str() == "test")
                    .and_then(|node| node.current_attempt)
                    .expect("matrix test attempt");
                self.queued.push_back(WorkflowEvent::Progress {
                    node: node_id("test"),
                    progress: WorkflowProgress {
                        attempt: test_attempt,
                        completed: Some(2),
                        total: Some(2),
                        message: "checks complete".into(),
                        detail: None,
                    },
                });
                for node in &mut self.snapshot.nodes {
                    if node.id.as_str() == "inspect" || node.id.as_str() == "test" {
                        node.state = NodeState::Terminal {
                            outcome: NodeTerminalState::Success,
                        };
                    }
                    if node.id.as_str() == "apply" {
                        let attempt = AttemptNumber::new(1).expect("valid matrix attempt");
                        node.state = NodeState::Running { attempt };
                        node.current_attempt = Some(attempt);
                    }
                }
                self.stage = MatrixStage::Mutating;
                Some(WorkflowEvent::Snapshot(self.snapshot.clone()))
            }
            MatrixStage::Mutating => {
                let apply = self
                    .snapshot
                    .nodes
                    .iter_mut()
                    .find(|node| node.id.as_str() == "apply")
                    .expect("matrix apply node");
                apply.state = NodeState::Terminal {
                    outcome: NodeTerminalState::Success,
                };
                apply.command_exit = Some(CommandExit::Code { code: 0 });
                apply.validated_output = Some(WorkflowValue::Bool(true));
                apply.artifacts.push(ArtifactReference {
                    kind: ArtifactKind::Stdout,
                    artifact: ArtifactRef {
                        relative_path: "artifacts/apply/stdout".into(),
                        retained_bytes: 5,
                        observed: crate::workflow::ArtifactObservation::Complete {
                            observed_bytes: 5,
                        },
                        digest: Digest(
                            "6742222222222222222222222222222222222222222222222222222222222222"
                                .into(),
                        ),
                    },
                });
                self.snapshot.lifecycle = RunLifecycle::Completed;
                self.snapshot.outcome = Some(WorkflowOutcome::Success);
                self.snapshot.exit_is_safe = true;
                self.stage = MatrixStage::Complete;
                self.queued.push_back(WorkflowEvent::Notice(
                    "workflow completed; durable state saved".into(),
                ));
                Some(WorkflowEvent::Snapshot(self.snapshot.clone()))
            }
            MatrixStage::Cancelling => {
                for node in &mut self.snapshot.nodes {
                    if matches!(node.state, NodeState::Running { .. } | NodeState::Ready) {
                        node.state = NodeState::Terminal {
                            outcome: NodeTerminalState::Cancellation,
                        };
                        node.terminal_reason = Some(TerminalReason::Cancellation(
                            "cancelled by terminal input".into(),
                        ));
                    }
                }
                self.snapshot.lifecycle = RunLifecycle::Completed;
                self.snapshot.outcome = Some(WorkflowOutcome::Cancellation);
                self.snapshot.cancellation = CancellationState::Saved;
                self.snapshot.exit_is_safe = true;
                self.stage = MatrixStage::Complete;
                self.queued.push_back(WorkflowEvent::Notice(format!(
                    "run saved; resume with rho workflow resume {}",
                    self.snapshot.run_id.expect("matrix run id")
                )));
                Some(WorkflowEvent::Snapshot(self.snapshot.clone()))
            }
        }
    }
}

#[cfg(debug_assertions)]
impl WorkflowEventAdapter for MatrixAdapter {
    fn session(&self) -> WorkflowSession {
        WorkflowSession::Owner
    }

    fn initial_snapshot(&self) -> WorkflowSnapshot {
        self.snapshot.clone()
    }

    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<WorkflowEvent>>> + Send + '_>> {
        Box::pin(async move {
            if let Some(event) = self.queued.pop_front() {
                return Ok(Some(event));
            }
            if matches!(
                self.stage,
                MatrixStage::AwaitingApproval | MatrixStage::Complete
            ) {
                std::future::pending::<()>().await;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            Ok(self.advance())
        })
    }

    fn send(
        &mut self,
        action: WorkflowAction,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match (self.stage, action) {
                (MatrixStage::AwaitingApproval, WorkflowAction::ConfirmPlan)
                    if self.start == MatrixWorkflowStart::Run =>
                {
                    self.begin();
                    Ok(())
                }
                (MatrixStage::AwaitingApproval, WorkflowAction::ConfirmResume)
                    if self.start == MatrixWorkflowStart::Resume =>
                {
                    self.begin();
                    Ok(())
                }
                (_, WorkflowAction::Cancel) => {
                    self.request_cancel();
                    Ok(())
                }
                _ => anyhow::bail!("workflow action is not valid in the current state"),
            }
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.request_cancel();
            if self.stage == MatrixStage::Cancelling {
                let _ = self.advance();
            }
            Ok(())
        })
    }
}

#[cfg(debug_assertions)]
fn matrix_snapshot(start: MatrixWorkflowStart) -> WorkflowSnapshot {
    let attempt_one = AttemptNumber::new(1).expect("valid matrix attempt");
    let inspect_state = if start == MatrixWorkflowStart::Resume {
        NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        }
    } else {
        NodeState::Pending
    };
    WorkflowSnapshot {
        workflow_name: "matrix workflow".into(),
        plan_id: PlanId::from_str(MATRIX_WORKFLOW_PLAN_ID).expect("valid matrix plan id"),
        run_id: Some(RunId::from_str(MATRIX_WORKFLOW_RUN_ID).expect("valid matrix run id")),
        graph_digest: Digest(
            "6740000000000000000000000000000000000000000000000000000000000000".into(),
        ),
        sources: SourceDigestSummary {
            source_count: 2,
            digest: Digest(
                "6741111111111111111111111111111111111111111111111111111111111111".into(),
            ),
        },
        approval: match start {
            MatrixWorkflowStart::Run => PlanApprovalState::AwaitingPlan,
            MatrixWorkflowStart::Resume => PlanApprovalState::AwaitingResume,
        },
        lifecycle: RunLifecycle::Planned,
        outcome: None,
        nodes: vec![
            WorkflowNodeSnapshot {
                id: node_id("inspect"),
                display_name: "Inspect workspace".into(),
                dependencies: Vec::new(),
                access: WorkspaceAccess::ReadOnly,
                execution: ExecutionMetadata::Agent {
                    name: "reviewer".into(),
                    runtime: AgentRuntime::Rho,
                    provider: Some("openai".into()),
                    model: Some("gpt-5.5".into()),
                },
                work: "Inspect the workspace and summarize risks".into(),
                state: inspect_state,
                current_attempt: (start == MatrixWorkflowStart::Resume).then_some(attempt_one),
                command_exit: None,
                validated_output: None,
                artifacts: Vec::new(),
                terminal_reason: None,
            },
            WorkflowNodeSnapshot {
                id: node_id("test"),
                display_name: "Run checks".into(),
                dependencies: Vec::new(),
                access: WorkspaceAccess::ReadOnly,
                execution: ExecutionMetadata::Command {
                    executable: "cargo".into(),
                    cwd: ".".into(),
                    shell: false,
                },
                work: "cargo test --workspace".into(),
                state: NodeState::Pending,
                current_attempt: None,
                command_exit: None,
                validated_output: None,
                artifacts: Vec::new(),
                terminal_reason: None,
            },
            WorkflowNodeSnapshot {
                id: node_id("apply"),
                display_name: "Apply result".into(),
                dependencies: vec![node_id("inspect"), node_id("test")],
                access: WorkspaceAccess::Mutating,
                execution: ExecutionMetadata::Command {
                    executable: "apply-result".into(),
                    cwd: ".".into(),
                    shell: false,
                },
                work: "apply-result --from inspect".into(),
                state: NodeState::Pending,
                current_attempt: None,
                command_exit: None,
                validated_output: None,
                artifacts: Vec::new(),
                terminal_reason: None,
            },
        ],
        cancellation: CancellationState::NotRequested,
        recovery_requirement: None,
        exit_is_safe: true,
    }
}

#[cfg(debug_assertions)]
fn node_id(value: &str) -> NodeId {
    NodeId::new(value).expect("valid matrix node id")
}
