use std::{future::Future, path::PathBuf, pin::Pin};

use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::workflow::{
    AttemptArtifacts, AttemptNumber, CancellationResumeState, CommandExit, Digest, NodeCompletion,
    NodeId, NodeTerminalState, RunId, TaskInstanceId, ValidatedOutputRef,
};

pub(crate) type WorkflowExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<NodeExecutionResult, RuntimeError>> + Send + 'a>>;

/// Executes one bound invocation of the indicated kind. Implementors must honor
/// cancellation and report uncertain cleanup rather than assume a process exited.
pub(crate) trait WorkflowNodeExecutor<I>: Send + Sync {
    fn execute<'a>(&'a self, request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a>;
}

/// Live activity from one node attempt for TUI and text observers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeProgressUpdate {
    pub(crate) message: String,
    pub(crate) detail: Option<String>,
    pub(crate) completed: Option<u64>,
    pub(crate) total: Option<u64>,
}

#[derive(Clone)]
pub(crate) struct NodeProgressReporter {
    node: TaskInstanceId,
    attempt: AttemptNumber,
    sender: UnboundedSender<RuntimeEvent>,
}

impl NodeProgressReporter {
    pub(crate) fn new(
        node: TaskInstanceId,
        attempt: AttemptNumber,
        sender: UnboundedSender<RuntimeEvent>,
    ) -> Self {
        Self {
            node,
            attempt,
            sender,
        }
    }

    pub(crate) fn report(&self, update: NodeProgressUpdate) {
        let _ = self.sender.send(RuntimeEvent::NodeProgress {
            node: self.node.clone(),
            attempt: self.attempt,
            message: update.message,
            detail: update.detail,
            completed: update.completed,
            total: update.total,
        });
    }

    pub(crate) fn message(&self, message: impl Into<String>) {
        self.report(NodeProgressUpdate {
            message: message.into(),
            detail: None,
            completed: None,
            total: None,
        });
    }
}

#[derive(Clone)]
pub(crate) struct NodeExecutionRequest<I> {
    pub(crate) invocation: super::prepared::PreparedInvocation<I>,
    pub(crate) plan_digest: Digest,
    pub(crate) run_id: RunId,
    pub(crate) node: TaskInstanceId,
    pub(crate) attempt: AttemptNumber,
    pub(crate) workspace: PathBuf,
    pub(crate) run_directory: PathBuf,
    pub(crate) attempt_directory: PathBuf,
    pub(crate) cancellation: rho_sdk::CancellationToken,
    pub(crate) progress: Option<NodeProgressReporter>,
}

impl<I> NodeExecutionRequest<I> {
    pub(super) fn map_execution<J>(self, map: impl FnOnce(I) -> J) -> NodeExecutionRequest<J> {
        NodeExecutionRequest {
            invocation: self.invocation.map_execution(map),
            plan_digest: self.plan_digest,
            run_id: self.run_id,
            node: self.node,
            attempt: self.attempt,
            workspace: self.workspace,
            run_directory: self.run_directory,
            attempt_directory: self.attempt_directory,
            cancellation: self.cancellation,
            progress: self.progress,
        }
    }

    pub(super) fn split_execution(self) -> (I, NodeExecutionRequest<()>) {
        let super::prepared::PreparedInvocation {
            execution,
            output,
            timeout_seconds,
            max_output_bytes,
        } = self.invocation;
        (
            execution,
            NodeExecutionRequest {
                invocation: super::prepared::PreparedInvocation {
                    execution: (),
                    output,
                    timeout_seconds,
                    max_output_bytes,
                },
                plan_digest: self.plan_digest,
                run_id: self.run_id,
                node: self.node,
                attempt: self.attempt,
                workspace: self.workspace,
                run_directory: self.run_directory,
                attempt_directory: self.attempt_directory,
                cancellation: self.cancellation,
                progress: self.progress,
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeExecutionResult {
    pub(crate) outcome: NodeTerminalState,
    pub(crate) command_exit: Option<CommandExit>,
    pub(crate) structured_output: Option<ValidatedOutputRef>,
    pub(crate) artifacts: AttemptArtifacts,
}

impl NodeExecutionResult {
    pub(crate) fn terminal(outcome: NodeTerminalState) -> Self {
        Self {
            outcome,
            command_exit: None,
            structured_output: None,
            artifacts: AttemptArtifacts::default(),
        }
    }

    pub(crate) fn completion(self, attempt: AttemptNumber) -> NodeCompletion {
        NodeCompletion {
            attempt: Some(attempt),
            outcome: self.outcome,
            cancellation_resume: (self.outcome == NodeTerminalState::Cancellation)
                .then_some(CancellationResumeState::Ready),
            command_exit: self.command_exit,
            structured_output: self.structured_output,
            artifacts: self.artifacts,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeSecurity {
    pub(crate) project_trusted: bool,
    pub(crate) permission_mode: crate::permission::PermissionMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum RuntimeEvent {
    StateChanged {
        revision: u64,
    },
    NodeStarted {
        node: TaskInstanceId,
        attempt: AttemptNumber,
    },
    /// In-flight activity for a launched node. Does not change durable state.
    NodeProgress {
        node: TaskInstanceId,
        attempt: AttemptNumber,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        completed: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
    },
    NodeFinished {
        node: TaskInstanceId,
        outcome: NodeTerminalState,
    },
    NeedsRecovery {
        nodes: Vec<TaskInstanceId>,
    },
    Completed,
}

impl RuntimeEvent {
    /// Canonical human-readable progress text for tools and CLI text output.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::StateChanged { revision } => format!("workflow state revision {revision}"),
            Self::NodeStarted { node, attempt } => {
                format!("workflow node {node} started attempt {attempt}")
            }
            Self::NodeProgress {
                node,
                message,
                detail,
                completed,
                total,
                ..
            } => {
                let mut text = format!("workflow node {node}: {message}");
                if let (Some(completed), Some(total)) = (completed, total) {
                    text = format!("{text} ({completed}/{total})");
                }
                if let Some(detail) = detail.as_deref().filter(|value| !value.is_empty()) {
                    text = format!("{text} · {detail}");
                }
                text
            }
            Self::NodeFinished { node, outcome } => {
                format!("workflow node {node} finished: {outcome:?}")
            }
            Self::NeedsRecovery { nodes } => format!(
                "workflow needs recovery: {}",
                nodes
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Completed => "workflow completed".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CleanupCause {
    Cancellation,
    Timeout,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeError {
    #[error(transparent)]
    Workflow(#[from] crate::workflow::WorkflowError),
    #[error("workflow workspace changed: planned '{planned}', current '{current}'")]
    WorkspaceChanged { planned: String, current: String },
    #[error("workflow node '{node}' requires project trust; create a new plan after trusting it")]
    TrustRemoved { node: NodeId },
    #[error("workflow node '{node}' launch metadata is missing or has the wrong kind")]
    LaunchMetadata { node: TaskInstanceId },
    #[error("workflow node '{node}' launch metadata is missing or has the wrong kind")]
    DefinitionLaunchMetadata { node: NodeId },
    #[error("workflow node '{node}' is not enforceably read-only: {capability}")]
    ReadOnlyCapability { node: NodeId, capability: String },
    #[error("workflow run needs explicit recovery for: {nodes}")]
    NeedsRecovery { nodes: String },
    #[error("workflow run is owned by another process")]
    ActiveOwner,
    #[error("workflow command was denied: {0}")]
    Denied(String),
    #[error("workflow operation was cancelled")]
    Cancelled,
    #[error(
        "workflow checkout lock wait budget exceeded: limit {wait_limit_seconds} seconds, waited at least {wait_limit_seconds} seconds"
    )]
    CheckoutLockTimeout { wait_limit_seconds: u64 },
    #[error("workflow executor cleanup did not confirm termination after {cause:?}")]
    CleanupUncertain { cause: CleanupCause },
    #[error("workflow artifact path is unsafe: {0}")]
    UnsafeArtifact(PathBuf),
    #[error("workflow runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("workflow runtime data is invalid: {0}")]
    Data(String),
    #[error("workflow executor failed: {0}")]
    Executor(String),
}

impl From<serde_json::Error> for RuntimeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Data(error.to_string())
    }
}
