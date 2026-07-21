use std::{fmt, path::PathBuf};

use crate::{
    AuthorizationDenialKind, CancellationToken, CapabilityRequest, HostInputRequest,
    HostInputResponse, Workspace,
};

use super::{
    Tool, ToolContext, ToolError, ToolFuture, ToolInvocation, ToolMetadata, ToolProgressSender,
};

/// Future returned while a [`Tool`](super::Tool) prepares one invocation.
pub type ToolPrepareFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<PreparedToolInvocation<'a>, ToolError>> + Send + 'a,
    >,
>;

/// Facts available while a tool validates and resolves an invocation.
///
/// Preparation cannot emit progress, ask for host input, or authorize work.
pub struct ToolPreparationContext {
    workspace: Option<Workspace>,
    cancellation: CancellationToken,
}

impl ToolPreparationContext {
    pub fn new(workspace: Option<Workspace>, cancellation: CancellationToken) -> Self {
        Self {
            workspace,
            cancellation,
        }
    }

    pub(crate) fn from_execution(execution: &ToolContext) -> Self {
        Self::new(
            execution.workspace().cloned(),
            execution.cancellation().clone(),
        )
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    pub fn workspace_root(&self) -> Option<&std::path::Path> {
        self.workspace.as_ref().map(Workspace::root)
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// Execution context for a resource-aware invocation whose declared
/// capabilities have already been authorized.
///
/// This type deliberately has no authorization method. A resource-aware
/// executor can use only the capabilities declared by its prepared invocation.
pub struct AuthorizedToolContext {
    execution: ToolContext,
}

impl AuthorizedToolContext {
    pub(crate) fn from_execution(execution: ToolContext) -> Self {
        Self { execution }
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.execution.workspace()
    }

    pub fn workspace_root(&self) -> Option<&std::path::Path> {
        self.execution.workspace_root()
    }

    pub fn cancellation(&self) -> &CancellationToken {
        self.execution.cancellation()
    }

    pub fn progress(&self) -> &ToolProgressSender {
        self.execution.progress()
    }

    pub async fn request_host_input(
        &self,
        request: HostInputRequest,
    ) -> Result<HostInputResponse, crate::Error> {
        self.execution.request_host_input(request).await
    }
}

/// Scheduler access mode for one resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolAccessMode {
    Shared,
    Exclusive,
}

/// Secret-safe category of a scheduler resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolResourceKind {
    WorkspacePath,
    DirectoryTree,
    DirectoryMembership,
    ManagedProcess,
    SessionState,
    ManagerState,
    ResponseStore,
    Opaque,
}

/// Resource identity used only to decide whether prepared calls may overlap.
///
/// Filesystem constructors expect canonical paths produced from the same
/// resolved workspace facts that execution will revalidate. Debug output omits
/// all resource values so paths, IDs, and tool-owned keys do not enter logs.
#[derive(Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ToolResource {
    WorkspacePath(PathBuf),
    DirectoryTree(PathBuf),
    DirectoryMembership(PathBuf),
    ManagedProcess(String),
    SessionState,
    ManagerState(String),
    ResponseStore(String),
    Opaque { owner: String, key: String },
}

impl ToolResource {
    pub fn workspace_path(canonical_path: impl Into<PathBuf>) -> Self {
        Self::WorkspacePath(canonical_path.into())
    }

    pub fn directory_tree(canonical_path: impl Into<PathBuf>) -> Self {
        Self::DirectoryTree(canonical_path.into())
    }

    pub fn directory_membership(canonical_path: impl Into<PathBuf>) -> Self {
        Self::DirectoryMembership(canonical_path.into())
    }

    pub fn managed_process(id: impl Into<String>) -> Self {
        Self::ManagedProcess(id.into())
    }

    pub fn session_state() -> Self {
        Self::SessionState
    }

    pub fn manager_state(owner: impl Into<String>) -> Self {
        Self::ManagerState(owner.into())
    }

    pub fn response_store(id: impl Into<String>) -> Self {
        Self::ResponseStore(id.into())
    }

    /// Creates a tool-owned resource. The owner namespace prevents accidental
    /// collisions between unrelated tools.
    pub fn opaque(owner: impl Into<String>, key: impl Into<String>) -> Self {
        Self::Opaque {
            owner: owner.into(),
            key: key.into(),
        }
    }

    pub fn kind(&self) -> ToolResourceKind {
        match self {
            Self::WorkspacePath(_) => ToolResourceKind::WorkspacePath,
            Self::DirectoryTree(_) => ToolResourceKind::DirectoryTree,
            Self::DirectoryMembership(_) => ToolResourceKind::DirectoryMembership,
            Self::ManagedProcess(_) => ToolResourceKind::ManagedProcess,
            Self::SessionState => ToolResourceKind::SessionState,
            Self::ManagerState(_) => ToolResourceKind::ManagerState,
            Self::ResponseStore(_) => ToolResourceKind::ResponseStore,
            Self::Opaque { .. } => ToolResourceKind::Opaque,
        }
    }
}

/// Runs the canonical prepared path from a tool's compatibility [`Tool::call`]
/// implementation.
///
/// Resource-aware tools can delegate `call` to this function instead of keeping
/// a second parsing, authorization, and execution path. The tool must override
/// [`Tool::prepare`], since its default implementation delegates back to
/// [`Tool::call`].
pub fn call_prepared<'a>(
    tool: &'a dyn Tool,
    invocation: ToolInvocation,
    execution: ToolContext,
) -> ToolFuture<'a> {
    Box::pin(async move {
        let preparation = ToolPreparationContext::from_execution(&execution);
        let prepared = tool.prepare(invocation, preparation).await?;
        for capability in prepared.capabilities() {
            execution
                .authorize(capability.clone())
                .await
                .map_err(|error| {
                    if matches!(error.kind(), AuthorizationDenialKind::Cancelled) {
                        ToolError::cancelled()
                    } else {
                        ToolError::policy_denied(&error)
                    }
                })?;
        }
        prepared.execute(execution).await
    })
}

impl fmt::Debug for ToolResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolResource")
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}

/// One access in a resource-aware execution policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResourceAccess {
    resource: ToolResource,
    mode: ToolAccessMode,
}

impl ToolResourceAccess {
    pub fn new(resource: ToolResource, mode: ToolAccessMode) -> Self {
        Self { resource, mode }
    }

    pub fn shared(resource: ToolResource) -> Self {
        Self::new(resource, ToolAccessMode::Shared)
    }

    pub fn exclusive(resource: ToolResource) -> Self {
        Self::new(resource, ToolAccessMode::Exclusive)
    }

    pub fn resource(&self) -> &ToolResource {
        &self.resource
    }

    pub fn mode(&self) -> ToolAccessMode {
        self.mode
    }
}

/// Scheduling contract for one prepared invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolExecutionPolicy {
    ResourceAware { accesses: Vec<ToolResourceAccess> },
    Exclusive,
}

impl ToolExecutionPolicy {
    pub fn resource_aware(accesses: impl IntoIterator<Item = ToolResourceAccess>) -> Self {
        Self::ResourceAware {
            accesses: accesses.into_iter().collect(),
        }
    }

    pub fn accesses(&self) -> Option<&[ToolResourceAccess]> {
        match self {
            Self::ResourceAware { accesses } => Some(accesses),
            Self::Exclusive => None,
        }
    }
}

type ExclusiveExecutor<'a> = Box<dyn FnOnce(ToolContext) -> ToolFuture<'a> + Send + 'a>;
type ResourceAwareExecutor<'a> =
    Box<dyn FnOnce(AuthorizedToolContext) -> ToolFuture<'a> + Send + 'a>;

enum ToolExecutor<'a> {
    Exclusive(ExclusiveExecutor<'a>),
    ResourceAware(ResourceAwareExecutor<'a>),
}

/// Validated, fully planned state for one tool invocation.
///
/// The executor consumes the prepared state exactly once. Resource-aware
/// executors receive [`AuthorizedToolContext`], which cannot request further
/// capabilities after the coordinator authorizes `capabilities()`.
pub struct PreparedToolInvocation<'a> {
    execution: ToolExecutionPolicy,
    capabilities: Vec<CapabilityRequest>,
    metadata: ToolMetadata,
    #[allow(dead_code)]
    executor: ToolExecutor<'a>,
}

impl<'a> PreparedToolInvocation<'a> {
    pub fn exclusive<F>(metadata: ToolMetadata, executor: F) -> Self
    where
        F: FnOnce(ToolContext) -> ToolFuture<'a> + Send + 'a,
    {
        Self {
            execution: ToolExecutionPolicy::Exclusive,
            capabilities: Vec::new(),
            metadata,
            executor: ToolExecutor::Exclusive(Box::new(executor)),
        }
    }

    pub fn resource_aware<F>(
        accesses: impl IntoIterator<Item = ToolResourceAccess>,
        capabilities: impl IntoIterator<Item = CapabilityRequest>,
        metadata: ToolMetadata,
        executor: F,
    ) -> Self
    where
        F: FnOnce(AuthorizedToolContext) -> ToolFuture<'a> + Send + 'a,
    {
        Self {
            execution: ToolExecutionPolicy::resource_aware(accesses),
            capabilities: capabilities.into_iter().collect(),
            metadata,
            executor: ToolExecutor::ResourceAware(Box::new(executor)),
        }
    }

    pub fn execution_policy(&self) -> &ToolExecutionPolicy {
        &self.execution
    }

    pub fn capabilities(&self) -> &[CapabilityRequest] {
        &self.capabilities
    }

    pub fn start_metadata(&self) -> &ToolMetadata {
        &self.metadata
    }

    #[allow(dead_code)]
    pub(crate) fn execute(self, execution: ToolContext) -> ToolFuture<'a> {
        match self.executor {
            ToolExecutor::Exclusive(executor) => executor(execution),
            ToolExecutor::ResourceAware(executor) => {
                executor(AuthorizedToolContext::from_execution(execution))
            }
        }
    }
}
