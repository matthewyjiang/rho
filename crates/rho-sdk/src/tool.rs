use std::{future::Future, path::Path, pin::Pin, sync::Arc};

use serde_json::Value;

use crate::{
    model::ToolSpec, AuthorizationError, AuthorizationOutcome, CancellationToken, CapabilityKind,
    CapabilityRequest, HostInputRequest, HostInputResponse, ToolCallId, Workspace,
};

mod arbiter;
mod first_capability;
mod output;
mod preparation;
mod progress;
mod registry;
pub(crate) mod scheduling;
mod worker;

pub use output::{ProcessResult, ToolAsset, ToolError, ToolErrorKind, ToolMetadata, ToolOutput};
pub use progress::{tool_progress_channel, ToolProgress, ToolProgressReceiver, ToolProgressSender};
pub use registry::{advertised_specs, DuplicateToolName, ToolRegistry, ToolVisibility};

pub(crate) use arbiter::ExecutionArbiter;
pub(crate) use first_capability::FirstCapability;
use preparation::call_prepared_for;
pub use preparation::{
    call_prepared, AuthorizedToolContext, PreparedToolInvocation, ToolAccessMode,
    ToolCancellationPolicy, ToolExecutionPolicy, ToolPreparationContext, ToolPrepareFuture,
    ToolResource, ToolResourceAccess, ToolResourceKind,
};
pub(crate) use worker::{begin_cancellation_cleanup, ToolHostWorker, ToolWorkerServices};

/// How the runtime delivers a tool's result to the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolExecutionMode {
    /// The loop waits for this call to finish before the next model request.
    #[default]
    Sync,
    /// The call may run detached: the loop keeps calling the model and delivers
    /// the result on the original call id when the job finishes.
    ///
    /// Automatic compaction is skipped while any async job is still pending
    /// (`tracing` warns with the pending count). Compacting with dangling
    /// tool calls is not supported.
    Async,
}

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

/// Trust origin of a registered tool implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolOrigin {
    /// In-process code supplied by the embedding host. SDK policy cannot sandbox it.
    HostProvided,
    /// A built-in adapter expected to authorize every declared capability.
    BuiltIn,
}

/// Static security declaration exposed before a tool is invoked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSecurity {
    origin: ToolOrigin,
    capabilities: Vec<CapabilityKind>,
}

impl ToolSecurity {
    pub fn host_provided() -> Self {
        Self {
            origin: ToolOrigin::HostProvided,
            capabilities: Vec::new(),
        }
    }

    pub fn built_in(capabilities: impl IntoIterator<Item = CapabilityKind>) -> Self {
        let mut capabilities = capabilities.into_iter().collect::<Vec<_>>();
        capabilities.sort();
        capabilities.dedup();
        Self {
            origin: ToolOrigin::BuiltIn,
            capabilities,
        }
    }

    pub fn origin(&self) -> ToolOrigin {
        self.origin
    }

    pub fn capabilities(&self) -> &[CapabilityKind] {
        &self.capabilities
    }
}

impl Default for ToolSecurity {
    fn default() -> Self {
        Self::host_provided()
    }
}

/// Actor that requested a tool invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolInvocationSource {
    /// The model returned the tool call in an assistant response.
    Model,
    /// The embedding host supplied the tool call before a model request.
    Host,
}

/// Owned input for one tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInvocation {
    id: ToolCallId,
    arguments: Value,
    source: ToolInvocationSource,
}

impl ToolInvocation {
    /// Creates a model-requested invocation.
    pub fn new(id: ToolCallId, arguments: Value) -> Self {
        Self {
            id,
            arguments,
            source: ToolInvocationSource::Model,
        }
    }

    pub(crate) fn from_host(id: ToolCallId, arguments: Value) -> Self {
        Self {
            id,
            arguments,
            source: ToolInvocationSource::Host,
        }
    }

    pub fn id(&self) -> &ToolCallId {
        &self.id
    }

    pub fn source(&self) -> ToolInvocationSource {
        self.source
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
    authorization: Arc<crate::workspace::AuthorizationServices>,
    host_input: Option<crate::host_input::HostInputRequester>,
    call_id: Option<ToolCallId>,
    cancellation: CancellationToken,
    progress: ToolProgressSender,
    first_capability: FirstCapability,
    detached: bool,
    invocation_source: ToolInvocationSource,
}

impl ToolContext {
    /// Creates a standalone host-originated context. SDK orchestration supplies
    /// the actual initiating source when attaching a context to an invocation.
    pub fn new(
        workspace: Option<Workspace>,
        cancellation: CancellationToken,
        progress: ToolProgressSender,
    ) -> Self {
        Self {
            workspace,
            authorization: Arc::new(crate::workspace::AuthorizationServices::denied()),
            host_input: None,
            call_id: None,
            cancellation,
            progress,
            first_capability: FirstCapability::default(),
            detached: false,
            invocation_source: ToolInvocationSource::Host,
        }
    }

    pub(crate) fn with_security(
        workspace: Option<Workspace>,
        authorization: Arc<crate::workspace::AuthorizationServices>,
        cancellation: CancellationToken,
        progress: ToolProgressSender,
    ) -> Self {
        Self {
            workspace,
            authorization,
            host_input: None,
            call_id: None,
            cancellation,
            progress,
            first_capability: FirstCapability::default(),
            detached: false,
            invocation_source: ToolInvocationSource::Host,
        }
    }

    /// The initiating actor, preserved when this call creates a child tool host.
    pub fn invocation_source(&self) -> ToolInvocationSource {
        self.invocation_source
    }

    pub(crate) fn with_invocation_source(mut self, source: ToolInvocationSource) -> Self {
        self.invocation_source = source;
        self
    }

    pub(crate) fn with_call_id(mut self, call_id: ToolCallId) -> Self {
        self.call_id = Some(call_id);
        self
    }

    pub(crate) fn with_host_input(
        mut self,
        host_input: crate::host_input::HostInputRequester,
    ) -> Self {
        self.host_input = Some(host_input);
        self
    }

    /// Marks this context as a detached async job. Host input is unsupported.
    pub(crate) fn detached(mut self) -> Self {
        self.host_input = None;
        self.detached = true;
        self
    }

    pub async fn request_host_input(
        &self,
        request: HostInputRequest,
    ) -> Result<HostInputResponse, crate::Error> {
        if self.detached {
            return Err(crate::Error::InvalidConfiguration {
                message: "detached async tools cannot request host input".into(),
            });
        }
        let requester =
            self.host_input
                .as_ref()
                .ok_or_else(|| crate::Error::InvalidConfiguration {
                    message: "tool context is not attached to an active run".into(),
                })?;
        requester.request(request).await
    }

    /// Creates child-run approval state routed through this call's active host.
    ///
    /// Call this once at the child-run boundary, then share the returned value
    /// across every child executor. The child uses this host call's handler,
    /// exact-request memory, and audit log.
    pub fn child_approval_session(&self) -> crate::ApprovalSession {
        self.authorization.approval_session()
    }

    pub(crate) fn authorization(&self) -> &crate::workspace::AuthorizationServices {
        &self.authorization
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace.as_ref().map(Workspace::root)
    }

    pub(crate) fn first_capability(&self) -> FirstCapability {
        self.first_capability.clone()
    }

    pub async fn authorize(
        &self,
        request: CapabilityRequest,
    ) -> Result<AuthorizationOutcome, AuthorizationError> {
        self.first_capability.record(&request);
        let capability = request.kind();
        tokio::select! {
            result = crate::workspace::authorize_for_call(
                &self.authorization,
                request,
                self.call_id.as_ref(),
                self.cancellation.clone(),
            ) => result,
            () = self.cancellation.cancelled() => {
                self.authorization.audit().record(
                    capability,
                    crate::ApprovalAuditDecision::Cancelled,
                );
                Err(AuthorizationError::cancelled(capability))
            },
        }
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn progress(&self) -> &ToolProgressSender {
        &self.progress
    }
}

/// Extension point for tools available to SDK sessions.
///
/// Implementors provide a stable JSON schema, use only capabilities explicitly
/// supplied through [`ToolContext`], cooperate with cancellation, and return a
/// `Send` future. Presentation data belongs in structured metadata rather than
/// preformatted terminal lines.
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    /// Declares trust origin and capabilities for diagnostics. Host-provided
    /// tools default to trusted in-process code with no SDK-enforced claims.
    fn security(&self) -> ToolSecurity {
        ToolSecurity::host_provided()
    }

    /// Declares that this tool reads [`crate::Session::live_history`] while it
    /// runs.
    ///
    /// Publishing the turn in flight copies the working history once per tool
    /// batch, so the runtime publishes it only when a registered tool declares
    /// the need. Without this declaration, `live_history` returns committed
    /// history only.
    fn reads_live_history(&self) -> bool {
        false
    }

    /// Async tools may be advertised as `async` to providers that support it; the
    /// runtime keeps calling the model while the job runs and delivers the result
    /// on the original call id. Async plans must be resource-aware with shared
    /// access only; host input is unavailable while detached.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sync
    }

    /// JSON Schema for retained successful [`ToolOutput::structured_content`].
    ///
    /// Not sent to providers: the model reads text content. Programmatic
    /// callers, such as script hosts that call tools, use it to document and
    /// validate successful structured results. Failed output is not constrained
    /// by this schema. Structured content may be absent, including when result
    /// budget limits discard it; callers must handle that separately.
    fn output_schema(&self) -> Option<Value> {
        None
    }

    /// Returns presentation metadata available before this tool starts.
    ///
    /// Implementors may derive metadata from validated or unvalidated arguments,
    /// but must not perform side effects or treat this hook as authorization.
    fn start_metadata(&self, _arguments: &Value) -> ToolMetadata {
        ToolMetadata::default()
    }

    /// Executes an authorized invocation.
    ///
    /// Implement this for a tool that runs exclusively and needs no resource
    /// plan. A tool that declares one implements [`Self::prepare`] instead and
    /// leaves this at its default, which resolves the plan and then executes
    /// it. Every tool must implement one of the two. Leaving both at their
    /// defaults fails the invocation with an explanatory error.
    ///
    /// The runtime enters through [`Self::prepare`], so this is called directly
    /// only by the default `prepare` and by hosts driving a tool by hand.
    /// Prepare-only out-of-tree tools therefore depend on this default body
    /// being present in the published `rho-sdk` version they compile against.
    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        call_prepared_for(self, invocation, context)
    }

    /// Validates and resolves an invocation before authorization and execution.
    ///
    /// The default retains the current [`Self::call`] path as an exclusive
    /// invocation. Existing tool implementations therefore remain compatible
    /// and cannot overlap another call unless they opt in with a complete
    /// resource-aware plan.
    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let metadata = self.start_metadata(invocation.arguments());
        Box::pin(async move {
            Ok(PreparedToolInvocation::from_default_prepare(
                metadata,
                move |execution| self.call(invocation, execution),
            ))
        })
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
