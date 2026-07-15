use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    model::ToolSpec, ApprovalHandler, CancellationToken, CapabilityRequest, DenyAllPolicy,
    DenyApprovals, ToolCallId, Workspace, WorkspacePolicy,
};

/// Future returned by [`Tool`] implementations.
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>>;

/// Structured operation category hosts may use for presentation and approval.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationKind {
    Read,
    Write,
    Execute,
    Network,
    Other(String),
}

/// Structured presentation metadata for a tool result or progress update.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolMetadata {
    operation: Option<OperationKind>,
    affected_paths: Vec<PathBuf>,
    command_summary: Option<String>,
    urls: Vec<String>,
    diff: Option<String>,
}

impl ToolMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn operation(mut self, operation: OperationKind) -> Self {
        self.operation = Some(operation);
        self
    }

    pub fn affected_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.affected_paths.push(path.into());
        self
    }

    pub fn command_summary(mut self, summary: impl Into<String>) -> Self {
        self.command_summary = Some(summary.into());
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.urls.push(url.into());
        self
    }

    pub fn diff(mut self, diff: impl Into<String>) -> Self {
        self.diff = Some(diff.into());
        self
    }

    pub fn operation_kind(&self) -> Option<&OperationKind> {
        self.operation.as_ref()
    }

    pub fn affected_paths(&self) -> &[PathBuf] {
        &self.affected_paths
    }

    pub fn command_summary_text(&self) -> Option<&str> {
        self.command_summary.as_deref()
    }

    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    pub fn unified_diff(&self) -> Option<&str> {
        self.diff.as_deref()
    }
}

/// Progress emitted during one tool invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolProgress {
    message: String,
    completed_units: Option<u64>,
    total_units: Option<u64>,
    metadata: ToolMetadata,
}

impl ToolProgress {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            completed_units: None,
            total_units: None,
            metadata: ToolMetadata::default(),
        }
    }

    pub fn units(mut self, completed: u64, total: u64) -> Self {
        self.completed_units = Some(completed);
        self.total_units = Some(total);
        self
    }

    pub fn metadata(mut self, metadata: ToolMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn text(&self) -> &str {
        &self.message
    }

    pub fn completed_units(&self) -> Option<u64> {
        self.completed_units
    }

    pub fn total_units(&self) -> Option<u64> {
        self.total_units
    }

    pub fn presentation(&self) -> &ToolMetadata {
        &self.metadata
    }
}

/// Sending side of a bounded tool-progress channel.
#[derive(Clone, Debug)]
pub struct ToolProgressSender {
    sender: mpsc::Sender<ToolProgress>,
}

impl ToolProgressSender {
    /// Sends progress with backpressure. Returns `false` if the host dropped it.
    pub async fn send(&self, progress: ToolProgress) -> bool {
        self.sender.send(progress).await.is_ok()
    }
}

/// Receiving side of a bounded tool-progress channel.
#[derive(Debug)]
pub struct ToolProgressReceiver {
    receiver: mpsc::Receiver<ToolProgress>,
}

impl ToolProgressReceiver {
    pub async fn recv(&mut self) -> Option<ToolProgress> {
        self.receiver.recv().await
    }

    pub(crate) fn try_recv(&mut self) -> Option<ToolProgress> {
        self.receiver.try_recv().ok()
    }
}

pub fn tool_progress_channel(capacity: NonZeroUsize) -> (ToolProgressSender, ToolProgressReceiver) {
    let (sender, receiver) = mpsc::channel(capacity.get());
    (
        ToolProgressSender { sender },
        ToolProgressReceiver { receiver },
    )
}

/// Owned input for one tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInvocation {
    id: ToolCallId,
    arguments: Value,
}

impl ToolInvocation {
    pub fn new(id: ToolCallId, arguments: Value) -> Self {
        Self { id, arguments }
    }

    pub fn id(&self) -> &ToolCallId {
        &self.id
    }

    pub fn arguments(&self) -> &Value {
        &self.arguments
    }

    pub fn into_arguments(self) -> Value {
        self.arguments
    }
}

/// Scoped capabilities supplied to one tool invocation.
#[derive(Clone, Debug)]
pub struct ToolContext {
    workspace: Option<Workspace>,
    policy: Arc<dyn WorkspacePolicy>,
    approvals: Arc<dyn ApprovalHandler>,
    cancellation: CancellationToken,
    progress: ToolProgressSender,
}

impl ToolContext {
    pub fn new(
        workspace: Option<Workspace>,
        cancellation: CancellationToken,
        progress: ToolProgressSender,
    ) -> Self {
        Self {
            workspace,
            policy: Arc::new(DenyAllPolicy),
            approvals: Arc::new(DenyApprovals),
            cancellation,
            progress,
        }
    }

    pub(crate) fn with_security(
        workspace: Option<Workspace>,
        policy: Arc<dyn WorkspacePolicy>,
        approvals: Arc<dyn ApprovalHandler>,
        cancellation: CancellationToken,
        progress: ToolProgressSender,
    ) -> Self {
        Self {
            workspace,
            policy,
            approvals,
            cancellation,
            progress,
        }
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace.as_ref().map(Workspace::root)
    }

    pub async fn authorize(&self, request: CapabilityRequest) -> Result<(), crate::Error> {
        tokio::select! {
            result = crate::workspace::authorize(&self.policy, &self.approvals, request) => result,
            () = self.cancellation.cancelled() => Err(crate::Error::Cancelled),
        }
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn progress(&self) -> &ToolProgressSender {
        &self.progress
    }
}

/// Successful structured tool output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolOutput {
    content: String,
    metadata: ToolMetadata,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            metadata: ToolMetadata::default(),
        }
    }

    pub fn metadata(mut self, metadata: ToolMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn presentation(&self) -> &ToolMetadata {
        &self.metadata
    }
}

/// Tool failure category independent of an implementation's internal errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolErrorKind {
    InvalidArguments,
    Execution,
    Cancelled,
}

/// Sanitized failure returned by a tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn cancelled() -> Self {
        Self::new(ToolErrorKind::Cancelled, "tool call cancelled")
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "tool failed: {}", self.message)
    }
}

impl std::error::Error for ToolError {}

/// Extension point for tools available to SDK sessions.
///
/// Implementors provide a stable JSON schema, use only capabilities explicitly
/// supplied through [`ToolContext`], cooperate with cancellation, and return a
/// `Send` future. Presentation data belongs in structured metadata rather than
/// preformatted terminal lines.
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a>;
}

/// Error returned when two tools use the same stable name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateToolName {
    name: String,
}

impl DuplicateToolName {
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for DuplicateToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "duplicate tool name '{}'", self.name)
    }
}

impl std::error::Error for DuplicateToolName {}

/// Deterministically ordered registry of SDK tools.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T) -> Result<(), DuplicateToolName>
    where
        T: Tool + 'static,
    {
        self.register_shared(Arc::new(tool))
    }

    pub fn register_shared(&mut self, tool: Arc<dyn Tool>) -> Result<(), DuplicateToolName> {
        let name = tool.spec().name;
        if self.tools.contains_key(&name) {
            return Err(DuplicateToolName { name });
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field("tool_names", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Deterministic outcome returned by [`ScriptedTool`].
#[derive(Clone, Debug)]
pub enum ScriptedToolOutcome {
    Success(ToolOutput),
    Failure(ToolError),
    WaitForCancellation,
}

/// Deterministic tool for downstream tests and examples.
#[derive(Clone, Debug)]
pub struct ScriptedTool {
    spec: ToolSpec,
    progress: Vec<ToolProgress>,
    outcome: ScriptedToolOutcome,
}

impl ScriptedTool {
    pub fn new(spec: ToolSpec, outcome: ScriptedToolOutcome) -> Self {
        Self {
            spec,
            progress: Vec::new(),
            outcome,
        }
    }

    pub fn progress(mut self, progress: impl IntoIterator<Item = ToolProgress>) -> Self {
        self.progress = progress.into_iter().collect();
        self
    }
}

impl Tool for ScriptedTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            for progress in &self.progress {
                if context.cancellation().is_cancelled() {
                    return Err(ToolError::cancelled());
                }
                context.progress().send(progress.clone()).await;
            }
            match &self.outcome {
                ScriptedToolOutcome::Success(output) => Ok(output.clone()),
                ScriptedToolOutcome::Failure(error) => Err(error.clone()),
                ScriptedToolOutcome::WaitForCancellation => {
                    context.cancellation().cancelled().await;
                    Err(ToolError::cancelled())
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
