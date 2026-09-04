use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
    task::Poll,
    time::Instant,
};

use tokio::{
    sync::{mpsc, Semaphore},
    task::JoinHandle,
};

use crate::{
    event::{ToolCompletion, ToolFailure},
    model::{Message, ToolCall, ToolResult},
    session::SessionCore,
    tool::{
        begin_cancellation_cleanup, tool_progress_channel, FirstCapability, ToolAccessMode,
        ToolCancellationPolicy, ToolContext, ToolError, ToolErrorKind, ToolExecutionMode,
        ToolExecutionPolicy, ToolInvocation, ToolOutput, ToolPreparationContext, ToolProgress,
        ToolRegistry,
    },
    CancellationToken, Error, RunEvent, ToolCallId,
};

use super::{
    emit, run_hooks::RunHooks, tool_batch::INTERRUPTED_TOOL_RESULT_CONTENT, Rho, RunControl,
};

pub(super) enum JobNotice {
    Progress {
        call_id: ToolCallId,
        progress: Box<ToolProgress>,
    },
    Finished(Box<FinishedJob>),
}

pub(super) struct FinishedJob {
    call_id: ToolCallId,
    name: String,
    completion: ToolCompletion,
    result: ToolResult,
    duration: Option<std::time::Duration>,
    capability: Option<crate::CapabilityRequest>,
}

pub(super) enum AwaitJobs {
    Continue,
    Cancelled,
}

struct AsyncJob {
    call: ToolCall,
    name: String,
    cancellation: CancellationToken,
    cancellation_policy: ToolCancellationPolicy,
    progress: crate::tool::ToolProgressReceiver,
    worker: JoinHandle<Result<ToolOutput, ToolError>>,
    started: Instant,
    first_capability: FirstCapability,
}

struct JobCompletion {
    call_id: ToolCallId,
    result: Result<ToolOutput, ToolError>,
}

/// Detached tool jobs for one run.
///
/// Completions are parked onto `finished` by [`forward_job_notice`];
/// [`Self::drain_finished`] appends those results to history.
pub(super) struct AsyncJobSet {
    jobs: BTreeMap<ToolCallId, AsyncJob>,
    /// Proposed calls not yet represented by either a live job or a parked result.
    unstarted: VecDeque<(ToolCallId, ToolCall)>,
    finished: VecDeque<ToolResult>,
    completions: mpsc::UnboundedReceiver<JobCompletion>,
    completions_tx: mpsc::UnboundedSender<JobCompletion>,
    execution_slots: Arc<Semaphore>,
}

impl AsyncJobSet {
    pub(super) fn new(max_parallel_tools: NonZeroUsize) -> Self {
        let (completions_tx, completions) = mpsc::unbounded_channel();
        Self {
            jobs: BTreeMap::new(),
            unstarted: VecDeque::new(),
            finished: VecDeque::new(),
            completions,
            completions_tx,
            execution_slots: Arc::new(Semaphore::new(max_parallel_tools.get())),
        }
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub(super) fn pending_count(&self) -> usize {
        self.jobs.len()
    }

    pub(super) fn drain_finished(&mut self, history: &mut Vec<Message>) -> usize {
        let count = self.finished.len();
        history.extend(self.finished.drain(..).map(Message::ToolResult));
        count
    }

    /// Collects every job whose worker is already finished without waiting.
    pub(super) fn harvest_ready(&mut self) -> Vec<JobNotice> {
        let mut notices = Vec::new();
        while let Ok(completion) = self.completions.try_recv() {
            if let Some(notice) = self.take_completion(completion) {
                notices.push(notice);
            }
        }
        notices
    }

    fn take_completion(&mut self, completion: JobCompletion) -> Option<JobNotice> {
        let job = self.jobs.remove(&completion.call_id)?;
        Some(finished_notice(job, completion.result))
    }

    pub(super) fn park_finished(&mut self, result: ToolResult) {
        self.finished.push_back(result);
    }

    pub(super) async fn poll_event(&mut self) -> JobNotice {
        std::future::poll_fn(|cx| loop {
            match self.completions.poll_recv(cx) {
                Poll::Ready(Some(completion)) => {
                    if let Some(notice) = self.take_completion(completion) {
                        return Poll::Ready(notice);
                    }
                    continue;
                }
                Poll::Ready(None) | Poll::Pending => {}
            }
            let ids = self.jobs.keys().cloned().collect::<Vec<_>>();
            for id in ids {
                let Some(job) = self.jobs.get_mut(&id) else {
                    continue;
                };
                if let Poll::Ready(Some(progress)) = job.progress.poll_recv(cx) {
                    return Poll::Ready(JobNotice::Progress {
                        call_id: id,
                        progress: Box::new(progress),
                    });
                }
            }
            return Poll::Pending;
        })
        .await
    }

    pub(super) async fn spawn(
        &mut self,
        calls: Vec<ToolCall>,
        core: &Arc<SessionCore>,
        runtime: &Rho,
        hooks: &RunHooks,
        events: &mpsc::Sender<RunEvent>,
        cancellation: &CancellationToken,
    ) -> Result<(), Error> {
        let calls = calls
            .into_iter()
            .map(|call| {
                let id = ToolCallId::from_string(call.id.clone())
                    .expect("validated provider tool call ID is nonempty");
                (id, call)
            })
            .collect::<Vec<_>>();
        if calls.is_empty() {
            return Ok(());
        }
        for (_, call) in &calls {
            if let Err(error) = emit(
                events,
                cancellation,
                RunEvent::ToolProposed { call: call.clone() },
            )
            .await
            {
                self.finished
                    .extend(calls.iter().map(|(_, call)| interrupted_result(call)));
                return Err(error);
            }
        }
        self.unstarted.extend(calls);
        let authorization = Arc::new(crate::workspace::AuthorizationServices::new(
            Arc::clone(&runtime.workspace_policy),
            Arc::clone(&runtime.approval_handler),
            core.approvals(),
            Arc::clone(&runtime.approval_audit),
            runtime.hooks.clone(),
            crate::workspace::AuthorizationScope {
                session_id: Some(core.id().clone()),
                run_id: Some(hooks.run_id().clone()),
                workspace_root: runtime
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.root().to_path_buf()),
                live_history: {
                    let core = Arc::clone(core);
                    Some(Arc::new(move || core.live_history()))
                },
            },
        ));
        while let Some((id, call)) = self.unstarted.pop_front() {
            let tool = runtime
                .tools
                .get(&call.name)
                .expect("split_tool_calls only routes registered async tools");
            let job_cancellation = CancellationToken::new();
            let (progress, progress_receiver) = tool_progress_channel(runtime.event_capacity);
            let context = ToolContext::with_security(
                runtime.workspace.clone(),
                Arc::clone(&authorization),
                job_cancellation.clone(),
                progress,
            )
            .with_call_id(id.clone())
            .detached();
            let first_capability = context.first_capability();
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let worker_tool = Arc::clone(&tool);
            let worker_call = call.clone();
            let worker_id = id.clone();
            let worker_cancellation = job_cancellation.clone();
            let completions_tx = self.completions_tx.clone();
            let completion_id = id.clone();
            let execution_slots = Arc::clone(&self.execution_slots);
            let worker = tokio::spawn(async move {
                let result = run_detached_job(
                    worker_tool,
                    worker_call,
                    worker_id,
                    context,
                    worker_cancellation,
                    execution_slots,
                    ready_tx,
                )
                .await;
                let _ = completions_tx.send(JobCompletion {
                    call_id: completion_id,
                    result: result.clone(),
                });
                result
            });
            self.jobs.insert(
                id.clone(),
                AsyncJob {
                    call: call.clone(),
                    name: tool.spec().name,
                    cancellation: job_cancellation.clone(),
                    cancellation_policy: ToolCancellationPolicy::Abort,
                    progress: progress_receiver,
                    worker,
                    started: Instant::now(),
                    first_capability: first_capability.clone(),
                },
            );
            let mut ready_rx = ready_rx;
            let ready = loop {
                tokio::select! {
                    biased;
                    result = &mut ready_rx => break result,
                    notice = self.poll_event() => {
                        forward_job_notice(notice, self, hooks, events, cancellation).await?;
                    }
                    () = cancellation.cancelled() => {
                        job_cancellation.cancel();
                        return Err(Error::Cancelled);
                    }
                }
            };
            match ready {
                Ok(Ok((metadata, cancellation_policy))) => {
                    self.jobs
                        .get_mut(&id)
                        .expect("new async job remains owned while starting")
                        .cancellation_policy = cancellation_policy;
                    emit(
                        events,
                        cancellation,
                        RunEvent::ToolStarted {
                            call_id: id.clone(),
                            name: call.name.clone(),
                            metadata,
                        },
                    )
                    .await?;
                    emit(events, cancellation, RunEvent::ToolDetached { call_id: id }).await?;
                }
                Ok(Err(error)) => {
                    let job = self
                        .jobs
                        .remove(&id)
                        .expect("failed async job remains owned while starting");
                    let _ = job.worker.await;
                    let completion = ToolCompletion::Failure(ToolFailure::new(
                        error.kind(),
                        error.message().to_owned(),
                    ));
                    let result = ToolResult {
                        id: call.id.clone(),
                        ok: false,
                        content: error.message().to_owned(),
                    };
                    fail_call(
                        hooks,
                        events,
                        cancellation,
                        &id,
                        &call,
                        completion,
                        result,
                        None,
                        first_capability.get().cloned(),
                        &mut self.finished,
                    )
                    .await?;
                }
                Err(_) => {
                    let job = self
                        .jobs
                        .remove(&id)
                        .expect("closed async job remains owned while starting");
                    let _ = job.worker.await;
                    let error = ToolError::new(
                        ToolErrorKind::Execution,
                        format!("async tool '{}' failed before detaching", call.name),
                    );
                    let completion = ToolCompletion::Failure(ToolFailure::new(
                        error.kind(),
                        error.message().to_owned(),
                    ));
                    let result = ToolResult {
                        id: call.id.clone(),
                        ok: false,
                        content: error.message().to_owned(),
                    };
                    fail_call(
                        hooks,
                        events,
                        cancellation,
                        &id,
                        &call,
                        completion,
                        result,
                        None,
                        first_capability.get().cloned(),
                        &mut self.finished,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn interrupt(
        &mut self,
        history: &mut Vec<Message>,
        hooks: &RunHooks,
        events: &mpsc::Sender<RunEvent>,
    ) {
        history.extend(
            std::mem::take(&mut self.unstarted)
                .into_iter()
                .map(|(_, call)| Message::ToolResult(interrupted_result(&call))),
        );
        self.drain_finished(history);
        let jobs = std::mem::take(&mut self.jobs);
        for job in jobs.values() {
            job.cancellation.cancel();
        }
        for (id, job) in jobs {
            let name = job.name.clone();
            let duration = Some(job.started.elapsed());
            let capability = job.first_capability.get().cloned();
            let result = settle_job(job).await;
            let completion = if result.ok {
                ToolCompletion::Success(crate::tool::ToolOutput::text(result.content.clone()))
            } else {
                ToolCompletion::Failure(ToolFailure::new(
                    ToolErrorKind::Cancelled,
                    result.content.clone(),
                ))
            };
            let _ = send_tool_finished(events, id.clone(), completion.clone()).await;
            hooks.after_tool_use(&name, &id, &completion, duration, capability.as_ref());
            history.push(Message::ToolResult(result));
        }
    }
}

fn async_plan_allowed(policy: &ToolExecutionPolicy) -> bool {
    match policy {
        ToolExecutionPolicy::Exclusive => false,
        ToolExecutionPolicy::ResourceAware { accesses } => accesses
            .iter()
            .all(|access| access.mode() == ToolAccessMode::Shared),
    }
}

fn finished_notice(job: AsyncJob, result: Result<ToolOutput, ToolError>) -> JobNotice {
    let duration = Some(job.started.elapsed());
    let capability = job.first_capability.get().cloned();
    let (completion, tool_result) = match result {
        Ok(output) => (
            ToolCompletion::Success(output.clone()),
            ToolResult {
                id: job.call.id.clone(),
                ok: true,
                content: output.content().to_owned(),
            },
        ),
        Err(error) => (
            ToolCompletion::Failure(ToolFailure::new(error.kind(), error.message().to_owned())),
            ToolResult {
                id: job.call.id.clone(),
                ok: false,
                content: error.message().to_owned(),
            },
        ),
    };
    JobNotice::Finished(Box::new(FinishedJob {
        call_id: ToolCallId::from_string(job.call.id.clone())
            .expect("validated provider tool call ID is nonempty"),
        name: job.name,
        completion,
        result: tool_result,
        duration,
        capability,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn fail_call(
    hooks: &RunHooks,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
    id: &ToolCallId,
    call: &ToolCall,
    completion: ToolCompletion,
    result: ToolResult,
    duration: Option<std::time::Duration>,
    capability: Option<crate::CapabilityRequest>,
    finished: &mut VecDeque<ToolResult>,
) -> Result<(), Error> {
    // Own the history result before any cancellable event publication. If a
    // host closes the event channel, terminal cleanup can still pair the call.
    finished.push_back(result);
    emit(
        events,
        cancellation,
        RunEvent::ToolStarted {
            call_id: id.clone(),
            name: call.name.clone(),
            metadata: Default::default(),
        },
    )
    .await?;
    let published = send_tool_finished(events, id.clone(), completion.clone()).await;
    hooks.after_tool_use(&call.name, id, &completion, duration, capability.as_ref());
    published?;
    if cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

/// Publishes the terminal half of a started call even after run cancellation.
async fn send_tool_finished(
    events: &mpsc::Sender<RunEvent>,
    call_id: ToolCallId,
    result: ToolCompletion,
) -> Result<(), Error> {
    events
        .send(RunEvent::ToolFinished { call_id, result })
        .await
        .map_err(|_| Error::Interrupted {
            message: "run event consumer was dropped".into(),
        })
}

async fn run_detached_job(
    tool: Arc<dyn crate::tool::Tool>,
    call: ToolCall,
    id: ToolCallId,
    context: ToolContext,
    cancellation: CancellationToken,
    execution_slots: Arc<Semaphore>,
    ready_tx: tokio::sync::oneshot::Sender<
        Result<(crate::tool::ToolMetadata, ToolCancellationPolicy), ToolError>,
    >,
) -> Result<ToolOutput, ToolError> {
    let invocation = ToolInvocation::new(id, call.arguments.clone());
    let workspace = context.workspace().cloned();
    let prepared = match tool
        .prepare(
            invocation,
            ToolPreparationContext::new(workspace, cancellation.clone()),
        )
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
    };
    if !async_plan_allowed(prepared.execution_policy()) {
        let error = ToolError::new(
            ToolErrorKind::Execution,
            format!(
                "async tool '{}' must declare a resource-aware plan with shared access only",
                call.name
            ),
        );
        let _ = ready_tx.send(Err(error.clone()));
        return Err(error);
    }
    for capability in prepared.capabilities() {
        if let Err(error) = context.authorize(capability.clone()).await {
            let error = if matches!(error.kind(), crate::AuthorizationDenialKind::Cancelled) {
                ToolError::cancelled()
            } else {
                ToolError::policy_denied(&error)
            };
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
    }
    let slot = tokio::select! {
        permit = execution_slots.acquire_owned() => {
            permit.expect("async execution semaphore is never closed")
        }
        () = cancellation.cancelled() => {
            let error = ToolError::cancelled();
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
    };
    let metadata = prepared.start_metadata().clone();
    let policy = prepared.cancellation_policy();
    if ready_tx.send(Ok((metadata, policy))).is_err() {
        return Err(ToolError::cancelled());
    }
    let _slot = slot;
    let cancellation_cleanup_timeout = Arc::new(Mutex::new(match policy {
        ToolCancellationPolicy::Abort => None,
        ToolCancellationPolicy::Complete { timeout } => Some(timeout),
    }));
    let execution = prepared.execute(context);
    tokio::pin!(execution);
    let mut cancellation_deferred = false;
    let mut cancellation_cleanup_deadline: Option<Pin<Box<tokio::time::Sleep>>> = None;
    loop {
        tokio::select! {
            biased;
            result = &mut execution => return result,
            () = async {
                cancellation_cleanup_deadline
                    .as_mut()
                    .expect("guarded cancellation cleanup deadline")
                    .await
            }, if cancellation_cleanup_deadline.is_some() => {
                return Err(ToolError::cancelled());
            }
            () = cancellation.cancelled(), if !cancellation_deferred => {
                let timeout = *cancellation_cleanup_timeout
                    .lock()
                    .expect("tool cancellation policy lock");
                begin_cancellation_cleanup(
                    timeout,
                    &mut cancellation_cleanup_deadline,
                    &mut cancellation_deferred,
                )?;
            }
        }
    }
}

async fn settle_job(mut job: AsyncJob) -> ToolResult {
    let result = match job.cancellation_policy {
        ToolCancellationPolicy::Complete { timeout } => {
            match tokio::time::timeout(timeout, &mut job.worker).await {
                Ok(result) => result,
                Err(_) => {
                    job.worker.abort();
                    let _ = (&mut job.worker).await;
                    return interrupted_result(&job.call);
                }
            }
        }
        ToolCancellationPolicy::Abort => {
            job.worker.abort();
            job.worker.await
        }
    };
    match result {
        Ok(Ok(output)) => ToolResult {
            id: job.call.id,
            ok: true,
            content: output.content().to_owned(),
        },
        Ok(Err(error)) if error.kind() == ToolErrorKind::Cancelled => interrupted_result(&job.call),
        Ok(Err(error)) => ToolResult {
            id: job.call.id,
            ok: false,
            content: error.message().to_owned(),
        },
        Err(_) => interrupted_result(&job.call),
    }
}

fn interrupted_result(call: &ToolCall) -> ToolResult {
    ToolResult {
        id: call.id.clone(),
        ok: false,
        content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
    }
}

pub(super) fn split_tool_calls(
    calls: Vec<ToolCall>,
    async_ids: &BTreeSet<String>,
    tools: &ToolRegistry,
) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let mut async_calls = Vec::new();
    let mut sync_calls = Vec::new();
    for call in calls {
        let declared_async = tools
            .get(&call.name)
            .is_some_and(|tool| tool.execution_mode() == ToolExecutionMode::Async);
        if async_ids.contains(&call.id) && declared_async {
            async_calls.push(call);
        } else {
            sync_calls.push(call);
        }
    }
    (async_calls, sync_calls)
}

pub(super) async fn forward_job_notice(
    notice: JobNotice,
    jobs: &mut AsyncJobSet,
    hooks: &RunHooks,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), Error> {
    match notice {
        JobNotice::Progress { call_id, progress } => {
            emit(
                events,
                cancellation,
                RunEvent::ToolUpdated {
                    call_id,
                    progress: *progress,
                },
            )
            .await
        }
        JobNotice::Finished(finished) => {
            let FinishedJob {
                call_id,
                name,
                completion,
                result,
                duration,
                capability,
            } = *finished;
            // The completion was removed from `jobs`; park its history result
            // before publishing anything that can be cancelled.
            jobs.park_finished(result);
            let published = send_tool_finished(events, call_id.clone(), completion.clone()).await;
            hooks.after_tool_use(&name, &call_id, &completion, duration, capability.as_ref());
            published?;
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            Ok(())
        }
    }
}

pub(super) async fn harvest_ready_jobs(control: &mut RunControl<'_>) -> Result<(), Error> {
    let notices = control.async_jobs.harvest_ready();
    for notice in notices {
        forward_job_notice(
            notice,
            control.async_jobs,
            control.hooks,
            control.events,
            control.cancellation,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn await_all_jobs(control: &mut RunControl<'_>) -> Result<(), Error> {
    while control.async_jobs.has_pending() {
        match await_first_job(control).await? {
            AwaitJobs::Continue => harvest_ready_jobs(control).await?,
            AwaitJobs::Cancelled => return Err(Error::Cancelled),
        }
    }
    Ok(())
}

pub(super) async fn await_first_job(control: &mut RunControl<'_>) -> Result<AwaitJobs, Error> {
    let mut commands_open = true;
    loop {
        tokio::select! {
            notice = control.async_jobs.poll_event() => {
                let finished = matches!(notice, JobNotice::Finished(_));
                forward_job_notice(
                    notice,
                    control.async_jobs,
                    control.hooks,
                    control.events,
                    control.cancellation,
                )
                .await?;
                if finished {
                    return Ok(AwaitJobs::Continue);
                }
            }
            command = control.commands.recv(), if commands_open => {
                match command {
                    Some(command) => {
                        super::accept_non_tool_command(command, control.steering);
                        if control.steering.has_staged() {
                            return Ok(AwaitJobs::Continue);
                        }
                    }
                    None => commands_open = false,
                }
            }
            () = control.cancellation.cancelled() => return Ok(AwaitJobs::Cancelled),
        }
    }
}

#[cfg(test)]
#[path = "async_jobs_tests.rs"]
mod tests;
