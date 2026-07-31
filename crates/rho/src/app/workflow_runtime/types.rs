use std::{collections::BTreeMap, future::Future, path::PathBuf, pin::Pin, sync::Arc};

use crate::workflow::{
    AttemptNumber, CommandExit, FrozenWorkflow, NodeId, NodeTerminalState, RunId, WorkflowValue,
};

pub(crate) type WorkflowExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<NodeExecutionResult, RuntimeError>> + Send + 'a>>;

/// Adapter boundary for Rho agents, Claude agents, and authorized commands.
pub(crate) trait WorkflowNodeExecutor: Send + Sync {
    fn execute<'a>(&'a self, request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a>;
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeExecutionResult {
    pub(crate) outcome: NodeTerminalState,
    pub(crate) command_exit: Option<CommandExit>,
    pub(crate) output: Option<WorkflowValue>,
}

impl NodeExecutionResult {
    pub(crate) fn terminal(outcome: NodeTerminalState) -> Self {
        Self {
            outcome,
            command_exit: None,
            output: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeSecurity {
    pub(crate) project_trusted: bool,
    pub(crate) permission_mode: crate::permission::PermissionMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeEvent {
    StateChanged {
        revision: u64,
    },
    NodeStarted {
        node: NodeId,
        attempt: AttemptNumber,
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
