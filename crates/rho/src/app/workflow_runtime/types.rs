use std::{collections::BTreeMap, future::Future, path::PathBuf, pin::Pin, sync::Arc};

use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::workflow::{
    AttemptArtifacts, AttemptNumber, CancellationResumeState, CommandExit, FrozenWorkflow,
    NodeCompletion, NodeId, NodeTerminalState, RunId, ValidatedOutputRef, WorkflowValue,
};

pub(crate) type WorkflowExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<NodeExecutionResult, RuntimeError>> + Send + 'a>>;

/// Adapter boundary for Rho agents, Claude agents, and authorized commands.
pub(crate) trait WorkflowNodeExecutor: Send + Sync {
    fn execute<'a>(&'a self, request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a>;
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
    node: NodeId,
    attempt: AttemptNumber,
    sender: UnboundedSender<RuntimeEvent>,
}

impl NodeProgressReporter {
    pub(crate) fn new(
        node: NodeId,
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
pub(crate) struct NodeExecutionRequest {
    pub(crate) workflow: Arc<FrozenWorkflow>,
    pub(crate) run_id: RunId,
    pub(crate) node: NodeId,
    pub(crate) attempt: AttemptNumber,
    pub(crate) workspace: PathBuf,
    pub(crate) attempt_directory: PathBuf,
    pub(crate) outputs: BTreeMap<NodeId, WorkflowValue>,
    pub(crate) cancellation: rho_sdk::CancellationToken,
    pub(crate) progress: Option<NodeProgressReporter>,
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
        node: NodeId,
        attempt: AttemptNumber,
    },
    /// In-flight activity for a launched node. Does not change durable state.
    NodeProgress {
        node: NodeId,
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
        node: NodeId,
        outcome: NodeTerminalState,
    },
    NeedsRecovery {
        nodes: Vec<NodeId>,
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
    LaunchMetadata { node: NodeId },
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
