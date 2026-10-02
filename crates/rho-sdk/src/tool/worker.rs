use std::{
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Instant,
};

use tokio::sync::mpsc;

use crate::{
    host_input::HostInputEnvelope,
    tool_host::{PendingToolHostInput, ToolHostCall, ToolHostEvent},
    CancellationToken, Error, Workspace,
};

use super::{
    Tool, ToolContext, ToolError, ToolErrorKind, ToolInvocation, ToolOutput,
    ToolPreparationContext, ToolRegistry,
};

/// Tool registry, workspace, and inherited authorization for a provider-free host.
pub(crate) struct ToolWorkerServices {
    pub tools: ToolRegistry,
    pub workspace: Option<Workspace>,
    pub authorization: Arc<crate::workspace::AuthorizationServices>,
    pub event_capacity: NonZeroUsize,
}

pub(crate) struct ToolHostWorker {
    pub core: Arc<ToolWorkerServices>,
    pub tool: Arc<dyn Tool>,
    pub call: ToolHostCall,
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
        let authorization = context.authorization().clone();
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
        authorization.after_tool_use(
            call.name(),
            call.call_id(),
            &result,
            started.elapsed(),
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
