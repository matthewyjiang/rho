use std::{
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Instant,
};

use tokio::sync::mpsc;

use crate::{
    hooks::{BoundedFailure, HookToolIdentity, HookToolStatus, HookWiring},
    host_input::HostInputEnvelope,
    tool_host::{PendingToolHostInput, ToolHostCall, ToolHostEvent},
    workspace::CapabilityRequest,
    ApprovalHandler, CancellationToken, Error, RunId, SessionId, Workspace, WorkspacePolicy,
};

use super::{
    Tool, ToolContext, ToolError, ToolErrorKind, ToolInvocation, ToolOutput,
    ToolPreparationContext, ToolRegistry,
};

/// Shared authorization, hook, and workspace services for a spawned tool worker.
///
/// [`crate::ToolHost`] builds this from its core. Orchestration builds it from
/// [`crate::Rho`] and session identity.
pub(crate) struct ToolWorkerServices {
    pub tools: ToolRegistry,
    pub workspace: Option<Workspace>,
    pub workspace_policy: Arc<dyn WorkspacePolicy>,
    pub approval_handler: Arc<dyn ApprovalHandler>,
    pub approvals: Arc<crate::workspace::SessionApprovals>,
    pub approval_audit: Arc<crate::workspace::ApprovalAuditLog>,
    pub hooks: HookWiring,
    pub event_capacity: NonZeroUsize,
    pub session_id: SessionId,
}

pub(crate) struct ToolHostWorker {
    pub core: Arc<ToolWorkerServices>,
    pub tool: Arc<dyn Tool>,
    pub call: ToolHostCall,
    pub run_id: RunId,
    pub context: ToolContext,
    pub cancellation: CancellationToken,
    pub events: mpsc::Sender<ToolHostEvent>,
    pub progress: super::ToolProgressReceiver,
    pub host_input: mpsc::Receiver<HostInputEnvelope>,
}

impl ToolHostWorker {
    pub(crate) async fn run(self) -> Result<ToolOutput, Error> {
        let Self {
            core,
            tool,
            call,
            run_id,
            context,
            cancellation,
            events,
            mut progress,
            mut host_input,
        } = self;
        let started = Instant::now();
        let invocation =
            ToolInvocation::from_host(call.call_id().clone(), call.arguments().clone());
        let workspace = core.workspace.clone();
        let first_capability = context.first_capability();
        let cancellation_cleanup_timeout = Arc::new(Mutex::new(None));
        let execution_completion = Arc::clone(&cancellation_cleanup_timeout);
        let execution = async {
            let prepared = tool
                .prepare(
                    invocation,
                    ToolPreparationContext::new(workspace, cancellation.clone()),
                )
                .await?;
            for capability in prepared.capabilities() {
                context
                    .authorize(capability.clone())
                    .await
                    .map_err(|error| {
                        if matches!(error.kind(), crate::AuthorizationDenialKind::Cancelled) {
                            ToolError::cancelled()
                        } else {
                            ToolError::policy_denied(&error)
                        }
                    })?;
            }
            *execution_completion
                .lock()
                .expect("tool cancellation policy lock") = match prepared.cancellation_policy() {
                crate::tool::ToolCancellationPolicy::Abort => None,
                crate::tool::ToolCancellationPolicy::Complete { timeout } => Some(timeout),
            };
            prepared.execute(context).await
        };
        tokio::pin!(execution);
        let mut progress_open = true;
        let mut host_input_open = true;
        let mut cancellation_deferred = false;
        let mut cancellation_cleanup_deadline: Option<Pin<Box<tokio::time::Sleep>>> = None;
        let result = loop {
            tokio::select! {
                biased;
                next = progress.recv(), if progress_open && !cancellation.is_cancelled() => {
                    if let Some(progress) = next {
                        if !send_event(&events, ToolHostEvent::Progress(progress), &cancellation).await {
                            let timeout = *cancellation_cleanup_timeout
                                .lock()
                                .expect("tool cancellation policy lock");
                            if let Err(error) = begin_cancellation_cleanup(
                                timeout,
                                &mut cancellation_cleanup_deadline,
                                &mut cancellation_deferred,
                            ) {
                                break Err(error);
                            }
                        }
                    } else {
                        progress_open = false;
                    }
                }
                next = host_input.recv(), if host_input_open && !cancellation.is_cancelled() => {
                    if let Some(envelope) = next {
                        let pending = PendingToolHostInput::from_envelope(envelope);
                        if !send_event(
                            &events,
                            ToolHostEvent::HostInputRequested(pending),
                            &cancellation,
                        )
                        .await
                        {
                            let timeout = *cancellation_cleanup_timeout
                                .lock()
                                .expect("tool cancellation policy lock");
                            if let Err(error) = begin_cancellation_cleanup(
                                timeout,
                                &mut cancellation_cleanup_deadline,
                                &mut cancellation_deferred,
                            ) {
                                break Err(error);
                            }
                        }
                    } else {
                        host_input_open = false;
                    }
                }
                result = &mut execution => break result,
                () = async {
                    cancellation_cleanup_deadline
                        .as_mut()
                        .expect("guarded cancellation cleanup deadline")
                        .await
                }, if cancellation_cleanup_deadline.is_some() => {
                    let timeout = cancellation_cleanup_timeout
                        .lock()
                        .expect("tool cancellation policy lock")
                        .expect("cleanup deadline requires a timeout");
                    break Err(ToolError::new(
                        ToolErrorKind::Cancelled,
                        format!("tool cancellation cleanup exceeded {timeout:?}"),
                    ));
                }
                () = cancellation.cancelled(), if !cancellation_deferred => {
                    let timeout = *cancellation_cleanup_timeout
                        .lock()
                        .expect("tool cancellation policy lock");
                    if let Err(error) = begin_cancellation_cleanup(
                        timeout,
                        &mut cancellation_cleanup_deadline,
                        &mut cancellation_deferred,
                    ) {
                        break Err(error);
                    }
                }
            }
        };
        while let Some(update) = progress.try_recv() {
            if !send_event(&events, ToolHostEvent::Progress(update), &cancellation).await {
                break;
            }
        }
        observe_after_tool_use(
            &core,
            &call,
            &run_id,
            &result,
            started,
            first_capability.get(),
        );
        result.map_err(Error::Tool)
    }
}

pub(crate) fn begin_cancellation_cleanup(
    timeout: Option<std::time::Duration>,
    deadline: &mut Option<Pin<Box<tokio::time::Sleep>>>,
    deferred: &mut bool,
) -> Result<(), ToolError> {
    let Some(timeout) = timeout else {
        return Err(ToolError::cancelled());
    };
    *deadline = Some(Box::pin(tokio::time::sleep(timeout)));
    *deferred = true;
    Ok(())
}

async fn send_event(
    sender: &mpsc::Sender<ToolHostEvent>,
    event: ToolHostEvent,
    cancellation: &CancellationToken,
) -> bool {
    tokio::select! {
        result = sender.send(event) => result.is_ok(),
        () = cancellation.cancelled() => false,
    }
}

fn observe_after_tool_use(
    core: &ToolWorkerServices,
    call: &ToolHostCall,
    run_id: &RunId,
    result: &Result<ToolOutput, ToolError>,
    started: Instant,
    capability: Option<&CapabilityRequest>,
) {
    let (status, failure) = match result {
        Ok(_) => (HookToolStatus::Succeeded, None),
        Err(error) => (
            HookToolStatus::Failed,
            Some(BoundedFailure {
                kind: tool_error_label(error.kind()),
                message: error.message(),
                field: "payload.failure",
            }),
        ),
    };
    core.hooks.observe_after_tool_use(
        HookToolIdentity {
            session_id: Some(&core.session_id),
            run_id: Some(run_id),
            workspace_root: core.workspace.as_ref().map(Workspace::root),
            tool_name: call.name(),
            call_id: call.call_id(),
        },
        status,
        failure,
        Some(started.elapsed().as_millis() as u64),
        capability,
    );
}

const fn tool_error_label(kind: ToolErrorKind) -> &'static str {
    match kind {
        ToolErrorKind::InvalidArguments => "invalid_arguments",
        ToolErrorKind::Execution => "execution",
        ToolErrorKind::PolicyDenied => "policy_denied",
        ToolErrorKind::Cancelled => "cancelled",
    }
}
