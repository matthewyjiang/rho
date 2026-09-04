use std::{future::Future, num::NonZeroUsize, pin::Pin, sync::Arc};

use serde_json::Value;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    hooks::{HookDelegation, HookHostLabels, HookPayloadBounds, HookWiring},
    host_input::HostInputEnvelope,
    tool::{
        tool_progress_channel, Tool, ToolContext, ToolHostWorker, ToolOutput, ToolProgress,
        ToolRegistry, ToolWorkerServices,
    },
    ApprovalAuditRecord, ApprovalHandler, ApprovalSession, CancellationToken, DenyAllPolicy,
    DenyApprovals, Error, HostInputRequest, HostInputResponse, RunId, SessionId, ToolCallId,
    Workspace, WorkspacePolicy,
};

/// Future returned by [`ToolHost::invoke`].
pub type ToolHostFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput, Error>> + Send + 'a>>;

/// One provider-free call of a registered tool.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolHostCall {
    id: ToolCallId,
    name: String,
    arguments: Value,
}

impl ToolHostCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: ToolCallId::new(),
            name: name.into(),
            arguments,
        }
    }

    pub fn id(mut self, id: ToolCallId) -> Self {
        self.id = id;
        self
    }

    pub fn call_id(&self) -> &ToolCallId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn arguments(&self) -> &Value {
        &self.arguments
    }

    fn validate(&self) -> Result<(), Error> {
        if self.name.trim().is_empty() {
            return Err(Error::InvalidConfiguration {
                message: "host tool name must not be empty".into(),
            });
        }
        if !self.arguments.is_object() {
            return Err(Error::InvalidConfiguration {
                message: "host tool arguments must be a JSON object".into(),
            });
        }
        Ok(())
    }
}

/// One event emitted while a provider-free tool call runs.
#[derive(Debug)]
#[non_exhaustive]
pub enum ToolHostEvent {
    Progress(ToolProgress),
    HostInputRequested(PendingToolHostInput),
}

/// Host question waiting for a response from a [`ToolHostRun`].
pub struct PendingToolHostInput {
    request: HostInputRequest,
    response: Option<tokio::sync::oneshot::Sender<Result<HostInputResponse, Error>>>,
}

impl PendingToolHostInput {
    pub(crate) fn from_envelope(envelope: HostInputEnvelope) -> Self {
        Self {
            request: envelope.request,
            response: Some(envelope.response),
        }
    }

    pub fn request(&self) -> &HostInputRequest {
        &self.request
    }

    /// Sends a validated response. An invalid response leaves the request open
    /// so the host can correct it and try again.
    pub fn respond(&mut self, response: HostInputResponse) -> Result<(), Error> {
        self.request.validate(&response)?;
        let sender = self
            .response
            .take()
            .ok_or_else(|| Error::InvalidHostResponse {
                message: "host input request was already answered".into(),
            })?;
        sender
            .send(Ok(response))
            .map_err(|_| Error::InvalidHostResponse {
                message: "tool call stopped before accepting host input".into(),
            })
    }
}

impl std::fmt::Debug for PendingToolHostInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingToolHostInput")
            .field("request_id", self.request.id())
            .field("answered", &self.response.is_none())
            .finish_non_exhaustive()
    }
}

/// Handle for one active provider-free tool call.
pub struct ToolHostRun {
    call_id: ToolCallId,
    cancellation: CancellationToken,
    events: mpsc::Receiver<ToolHostEvent>,
    worker: Option<JoinHandle<Result<ToolOutput, Error>>>,
    finished: bool,
}

impl ToolHostRun {
    fn new(
        call_id: ToolCallId,
        cancellation: CancellationToken,
        events: mpsc::Receiver<ToolHostEvent>,
        worker: JoinHandle<Result<ToolOutput, Error>>,
    ) -> Self {
        Self {
            call_id,
            cancellation,
            events,
            worker: Some(worker),
            finished: false,
        }
    }

    pub fn call_id(&self) -> &ToolCallId {
        &self.call_id
    }

    pub fn cancellation_handle(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub async fn next_event(&mut self) -> Option<ToolHostEvent> {
        self.events.recv().await
    }

    /// Waits for the tool output while draining any unconsumed progress.
    /// Host-input events are dropped, which makes an unanswered tool request
    /// fail rather than waiting forever.
    pub async fn outcome(&mut self) -> Result<ToolOutput, Error> {
        let mut worker = self
            .worker
            .take()
            .ok_or_else(|| Error::InvalidHostResponse {
                message: "tool host outcome was already consumed".into(),
            })?;
        let result = loop {
            tokio::select! {
                result = &mut worker => {
                    break result.map_err(|error| Error::Interrupted {
                        message: format!("tool host task failed: {error}"),
                    })?;
                }
                event = self.events.recv() => {
                    if event.is_none() {
                        break worker.await.map_err(|error| Error::Interrupted {
                            message: format!("tool host task failed: {error}"),
                        })?;
                    }
                }
            }
        };
        self.finished = true;
        result
    }
}

impl Drop for ToolHostRun {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.cancellation.cancel();
        // Detach the worker so it can observe cancellation and pair its
        // BeforeToolUse hook with AfterToolUse before it ends.
        self.worker.take();
    }
}

impl std::fmt::Debug for ToolHostRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolHostRun")
            .field("call_id", &self.call_id)
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

/// Builder for a provider-free [`ToolHost`].
#[derive(Default)]
pub struct ToolHostBuilder {
    tools: Vec<Arc<dyn Tool>>,
    workspace: Option<Workspace>,
    workspace_policy: Option<Arc<dyn WorkspacePolicy>>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    approval_session: Option<ApprovalSession>,
    event_capacity: Option<NonZeroUsize>,
    hook_observer: Option<Arc<dyn crate::hooks::HookObserver>>,
    pre_tool_gate: Option<Arc<dyn crate::hooks::PreToolUseGate>>,
    hook_payload_bounds: HookPayloadBounds,
    hook_delegation: HookDelegation,
    hook_host_labels: HookHostLabels,
    session_id: Option<SessionId>,
}

impl ToolHostBuilder {
    pub fn tool<T>(mut self, tool: T) -> Self
    where
        T: Tool + 'static,
    {
        self.tools.push(Arc::new(tool));
        self
    }

    pub fn tool_shared(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn workspace(mut self, workspace: Workspace) -> Self {
        self.workspace = Some(workspace);
        self
    }

    pub fn workspace_policy<P>(mut self, policy: P) -> Self
    where
        P: WorkspacePolicy + 'static,
    {
        self.workspace_policy = Some(Arc::new(policy));
        self
    }

    pub fn approval_handler<A>(mut self, handler: A) -> Self
    where
        A: ApprovalHandler + 'static,
    {
        self.approval_handler = Some(Arc::new(handler));
        self.approval_session = None;
        self
    }

    pub fn approval_handler_shared(mut self, handler: Arc<dyn ApprovalHandler>) -> Self {
        self.approval_handler = Some(handler);
        self.approval_session = None;
        self
    }

    /// Shares one exact-request approval session with other runtimes or hosts.
    pub fn approval_session(mut self, session: ApprovalSession) -> Self {
        self.approval_session = Some(session);
        self.approval_handler = None;
        self
    }

    pub fn event_capacity(mut self, capacity: NonZeroUsize) -> Self {
        self.event_capacity = Some(capacity);
        self
    }

    pub fn hook_observer_shared(mut self, observer: Arc<dyn crate::hooks::HookObserver>) -> Self {
        self.hook_observer = Some(observer);
        self
    }

    pub fn pre_tool_gate_shared(mut self, gate: Arc<dyn crate::hooks::PreToolUseGate>) -> Self {
        self.pre_tool_gate = Some(gate);
        self
    }

    pub fn hook_payload_bounds(mut self, bounds: HookPayloadBounds) -> Self {
        self.hook_payload_bounds = bounds;
        self
    }

    pub fn hook_delegation(mut self, delegation: HookDelegation) -> Self {
        self.hook_delegation = delegation;
        self
    }

    pub fn hook_host_labels(mut self, labels: HookHostLabels) -> Self {
        self.hook_host_labels = labels;
        self
    }

    pub fn session_id(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn build(self) -> Result<ToolHost, Error> {
        let mut tools = ToolRegistry::new();
        for tool in self.tools {
            tools
                .register_shared(tool)
                .map_err(|error| Error::InvalidConfiguration {
                    message: error.to_string(),
                })?;
        }
        let approval_session = self.approval_session.unwrap_or_else(|| {
            ApprovalSession::from_shared(
                self.approval_handler
                    .unwrap_or_else(|| Arc::new(DenyApprovals)),
            )
        });
        Ok(ToolHost {
            core: Arc::new(ToolWorkerServices {
                tools,
                workspace: self.workspace,
                workspace_policy: self
                    .workspace_policy
                    .unwrap_or_else(|| Arc::new(DenyAllPolicy)),
                approval_handler: approval_session.handler(),
                approvals: approval_session.remembered(),
                approval_audit: approval_session.audit_log(),
                hooks: HookWiring::new(
                    self.hook_observer,
                    self.pre_tool_gate,
                    self.hook_payload_bounds,
                    self.hook_delegation,
                )
                .with_host_labels(self.hook_host_labels),
                event_capacity: self.event_capacity.unwrap_or_else(|| {
                    NonZeroUsize::new(crate::client::DEFAULT_EVENT_CAPACITY).unwrap()
                }),
                session_id: self.session_id.unwrap_or_default(),
            }),
        })
    }
}

type ToolHostCore = ToolWorkerServices;

/// Provider-free host for registered SDK tools.
///
/// A host owns one logical authorization session. Calls share remembered
/// `AllowForSession` decisions and a bounded approval audit. Capability-bearing
/// tools always use the SDK order: workspace policy, `before_tool_use`, host
/// approval when required, then execution.
#[derive(Clone)]
pub struct ToolHost {
    core: Arc<ToolHostCore>,
}

impl ToolHost {
    pub fn builder() -> ToolHostBuilder {
        ToolHostBuilder::default()
    }

    pub fn session_id(&self) -> &SessionId {
        &self.core.session_id
    }

    pub fn tool_specs(&self) -> Vec<crate::model::ToolSpec> {
        self.core.tools.specs()
    }

    pub fn approval_audit(&self) -> Vec<ApprovalAuditRecord> {
        self.core.approval_audit.snapshot()
    }

    /// Starts one tool call without a model provider.
    pub fn start(&self, call: ToolHostCall) -> Result<ToolHostRun, Error> {
        call.validate()?;
        let tool = self
            .core
            .tools
            .get(call.name())
            .ok_or_else(|| Error::InvalidConfiguration {
                message: format!("tool '{}' is not registered", call.name()),
            })?;
        let run_id = RunId::new();
        let cancellation = CancellationToken::new();
        let (events_sender, events) = mpsc::channel(self.core.event_capacity.get());
        let (progress, progress_receiver) = tool_progress_channel(self.core.event_capacity);
        let (host_input, host_input_receiver) =
            crate::host_input::channel(self.core.event_capacity.get(), cancellation.clone());
        let authorization = Arc::new(crate::workspace::AuthorizationServices::new(
            Arc::clone(&self.core.workspace_policy),
            Arc::clone(&self.core.approval_handler),
            Arc::clone(&self.core.approvals),
            Arc::clone(&self.core.approval_audit),
            self.core.hooks.clone(),
            crate::workspace::AuthorizationScope {
                session_id: Some(self.core.session_id.clone()),
                run_id: Some(run_id.clone()),
                workspace_root: self
                    .core
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.root().to_path_buf()),
                live_history: None,
            },
        ));
        let context = ToolContext::with_security(
            self.core.workspace.clone(),
            authorization,
            cancellation.clone(),
            progress,
        )
        .with_call_id(call.id.clone())
        .with_host_input(host_input);
        let worker = tokio::spawn(
            ToolHostWorker {
                core: Arc::clone(&self.core),
                tool,
                call: call.clone(),
                run_id,
                context,
                cancellation: cancellation.clone(),
                events: events_sender,
                progress: progress_receiver,
                host_input: host_input_receiver,
            }
            .run(),
        );
        Ok(ToolHostRun::new(call.id, cancellation, events, worker))
    }

    /// Runs one non-interactive tool call and drains its progress events.
    /// Use [`Self::start`] when the tool can ask the host for input.
    pub fn invoke(&self, call: ToolHostCall) -> ToolHostFuture<'_> {
        Box::pin(async move {
            let mut run = self.start(call)?;
            run.outcome().await
        })
    }
}

impl std::fmt::Debug for ToolHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolHost")
            .field("session_id", &self.core.session_id)
            .field("tools", &self.core.tools)
            .field(
                "workspace_root",
                &self.core.workspace.as_ref().map(Workspace::root),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "tool_host_tests.rs"]
mod tests;
