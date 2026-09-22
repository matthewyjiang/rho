use std::{collections::BTreeMap, sync::Arc, time::Instant};

use tokio::task::JoinSet;

use crate::workflow::{
    attempt_directory, next_actions, AttemptNumber, AttemptRecord, AttemptState, ExternalOwner,
    NodeTerminalState, RunId, RunLifecycle, SchedulerAction, SchedulerCapacity, StoredRun,
    TaskInstanceId, WorkflowEvent, WorkflowStore, WorkspaceAccess, ATTEMPT_VERSION,
};

use super::{
    artifacts::write_json,
    cancellation::{
        cancel_waiting_nodes, latest_cancellation_request, latest_pending_cancellation_request,
        read_cancellation_request, run_directory, CROSS_PROCESS_CANCEL_POLL,
    },
    journal::RunJournal,
    prepared::{PreparedExecution, PreparedInvocation},
    recovery::{mark_attempt_uncertain, recover_completed_transitions, recover_state, ResumePlan},
    runner::{send_event, RecoveryDecision, WorkflowRunner},
    CheckoutGate, CleanupCause, NodeExecutionRequest, NodeExecutionResult, NodeProgressReporter,
    RuntimeError, RuntimeEvent,
};

struct NodeTaskOutput {
    node: TaskInstanceId,
    attempt: AttemptNumber,
    result: Result<NodeExecutionResult, RuntimeError>,
}

enum DriveStart<'a> {
    Finished(Box<StoredRun>),
    Running(Box<DriveSession<'a>>),
}

enum TaskWait {
    Continue,
    Joined(Box<NodeTaskOutput>),
}

struct DriveSession<'a> {
    runner: &'a WorkflowRunner,
    journal: RunJournal,
    drive_started_at: Instant,
    attempt_started_at: BTreeMap<TaskInstanceId, Instant>,
    events: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    cancellation_request_id: Option<String>,
    checkout: CheckoutGate,
    tasks: JoinSet<Result<NodeTaskOutput, RuntimeError>>,
}

pub(super) async fn drive(
    runner: &WorkflowRunner,
    run_id: RunId,
    recovery: RecoveryDecision,
    events: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
) -> Result<StoredRun, RuntimeError> {
    match DriveSession::bootstrap_run(runner, run_id, recovery, events)? {
        DriveStart::Finished(run) => Ok(*run),
        DriveStart::Running(session) => session.run_loop().await,
    }
}

impl<'a> DriveSession<'a> {
    fn bootstrap_run(
        runner: &'a WorkflowRunner,
        run_id: RunId,
        recovery: RecoveryDecision,
        events: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> Result<DriveStart<'a>, RuntimeError> {
        let store = WorkflowStore::new(&runner.rho_home)?;
        let mut guard = store.lock_run(run_id).map_err(|error| {
            if error.to_string().contains("active writer") {
                RuntimeError::ActiveOwner
            } else {
                RuntimeError::Workflow(error)
            }
        })?;
        let (mut run, records) = store.load_run_with_events(run_id)?;
        let drive_started_at = Instant::now();
        let directory = run_directory(&runner.rho_home, run_id);
        // load_run validates the saved prefix, not the tail. Use the same domain
        // reducer for the authoritative tail and reuse these events for cancellation.
        let cancellation_request_id = latest_cancellation_request(&records);
        let pending_cancellation = latest_pending_cancellation_request(&records);
        let tail = records.last().map_or(0, |record| record.sequence);
        if tail != run.state.last_event_sequence {
            run.state.state =
                crate::workflow::derive_snapshot(&run.graph, &records, tail, &directory)?;
            run.state.last_event_sequence = tail;
            store.save_state(&mut guard, &run.state)?;
        }
        let mut journal = RunJournal {
            store,
            guard,
            directory,
            run,
        };
        recover_completed_transitions(&mut journal)?;
        let first_start = journal.run.state.state.lifecycle == RunLifecycle::Planned;
        runner.validate_security(&journal.run)?;
        let checkout = CheckoutGate::new(&runner.rho_home, &runner.workspace)?;
        let plan = ResumePlan::for_run(&journal.run.state);
        if matches!(plan, ResumePlan::Finished) {
            return Ok(DriveStart::Finished(Box::new(journal.run)));
        }
        recover_state(
            &mut journal,
            plan,
            recovery,
            cancellation_request_id.as_deref(),
            pending_cancellation,
            &events,
        )?;
        if first_start {
            if let Some(hooks) = &runner.hooks {
                journal.commit(WorkflowEvent::HookObserved {
                    event: "workflow_started".into(),
                    node: None,
                    attempt: None,
                })?;
                hooks.observe_workflow_started(
                    &run_id.to_string(),
                    &journal.run.manifest.program_digest.0,
                );
            }
        }
        Ok(DriveStart::Running(Box::new(Self {
            runner,
            journal,
            drive_started_at,
            attempt_started_at: BTreeMap::new(),
            events,
            cancellation_request_id,
            checkout,
            tasks: JoinSet::new(),
        })))
    }

    async fn run_loop(mut self) -> Result<StoredRun, RuntimeError> {
        loop {
            self.handle_cancellation_edge()?;
            if self.journal.run.state.state.cancellation_requested {
                cancel_waiting_nodes(&mut self.journal)?;
            }
            let capacity = available_capacity(&self.journal.run.graph);
            let launched = self.handle_scheduler_actions(capacity)?;
            if launched
                || !next_actions(
                    &self.journal.run.graph,
                    &self.journal.run.state.state,
                    capacity,
                )?
                .is_empty()
            {
                continue;
            }
            if self.tasks.is_empty() {
                return self.finish();
            }
            match self.await_next_task().await? {
                TaskWait::Continue => continue,
                TaskWait::Joined(joined) => self.complete_node(*joined)?,
            }
        }
    }

    fn handle_cancellation_edge(&mut self) -> Result<(), RuntimeError> {
        let run_id = self.journal.run.manifest.run_id;
        let durable_cancellation = read_cancellation_request(&self.journal.store, run_id)?;
        if self.runner.cancellation.is_cancelled() || durable_cancellation.is_some() {
            if !self.journal.run.state.state.cancellation_requested {
                let request_id =
                    durable_cancellation.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                self.cancellation_request_id = Some(request_id.clone());
                self.journal.commit_and_notify(
                    WorkflowEvent::CancellationRequested { request_id },
                    &self.events,
                )?;
            }
            self.runner.cancellation.cancel();
        }
        Ok(())
    }

    fn handle_scheduler_actions(
        &mut self,
        capacity: SchedulerCapacity,
    ) -> Result<bool, RuntimeError> {
        let actions = next_actions(
            &self.journal.run.graph,
            &self.journal.run.state.state,
            capacity,
        )?;
        let mut launched = false;
        for action in actions {
            match action {
                SchedulerAction::MarkReady { node } => {
                    self.journal
                        .commit_and_notify(WorkflowEvent::NodeReady { node }, &self.events)?;
                }
                SchedulerAction::MarkTerminal { node, outcome } => {
                    self.journal.commit_and_notify(
                        WorkflowEvent::NodeFinished {
                            node: node.clone(),
                            completion: Box::new(crate::workflow::NodeCompletion::terminal(
                                outcome,
                            )),
                        },
                        &self.events,
                    )?;
                    send_event(&self.events, RuntimeEvent::NodeFinished { node, outcome });
                }
                SchedulerAction::Launch { node, access } => {
                    self.launch_node(node, access)?;
                    launched = true;
                }
                SchedulerAction::FinishScope { scope, result } => {
                    self.journal.commit_and_notify(
                        WorkflowEvent::ScopeFinished { scope, result },
                        &self.events,
                    )?;
                }
            }
        }
        Ok(launched)
    }

    fn launch_node(
        &mut self,
        node: TaskInstanceId,
        access: WorkspaceAccess,
    ) -> Result<(), RuntimeError> {
        // Binding cannot launch external work. Keep a failed binding as an attempt
        // so its diagnostic artifact is owned by a durable completion, not only a
        // transient progress message. No new failure-only wire variant is needed.
        let invocation = PreparedInvocation::prepare(
            &self.journal.run.graph,
            &node,
            &self.journal.run.state.state,
        );
        let run_id = self.journal.run.manifest.run_id;
        let attempt = self.journal.run.state.state.next_attempt(&node)?;
        self.journal.commit(WorkflowEvent::LaunchIntended {
            node: node.clone(),
            attempt,
        })?;
        let attempt_directory = attempt_directory(&self.journal.directory, &node, attempt);
        let relative_attempt = attempt_directory
            .strip_prefix(&self.journal.directory)
            .map_err(|_| RuntimeError::UnsafeArtifact(attempt_directory.clone()))?;
        crate::workflow::ensure_directory_beneath(&self.journal.directory, relative_attempt)?;
        write_attempt(
            &self.journal.directory,
            &attempt_directory,
            attempt,
            AttemptState::LaunchIntended,
        )?;
        let owner = ExternalOwner::Process {
            pid: std::process::id(),
        };
        write_attempt(
            &self.journal.directory,
            &attempt_directory,
            attempt,
            AttemptState::Started {
                owner: owner.clone(),
            },
        )?;
        self.journal.commit(WorkflowEvent::AttemptStarted {
            node: node.clone(),
            attempt,
            owner,
        })?;
        self.attempt_started_at.insert(node.clone(), Instant::now());
        if let Some(hooks) = &self.runner.hooks {
            self.journal.commit(WorkflowEvent::HookObserved {
                event: "workflow_node_started".into(),
                node: Some(node.clone()),
                attempt: Some(attempt),
            })?;
            hooks.observe_workflow_node_started(
                &run_id.to_string(),
                &self.journal.run.manifest.program_digest.0,
                &node.to_string(),
                attempt.get(),
            );
        }
        self.journal.notify(&self.events);
        send_event(
            &self.events,
            RuntimeEvent::NodeStarted {
                node: node.clone(),
                attempt,
            },
        );
        let invocation = match invocation {
            Ok(invocation) => invocation,
            Err(error) => {
                return self.complete_node(NodeTaskOutput {
                    node,
                    attempt,
                    result: Err(error),
                })
            }
        };
        let gate = self.checkout.clone();
        let progress = self
            .events
            .as_ref()
            .map(|sender| NodeProgressReporter::new(node.clone(), attempt, sender.clone()));
        // AttemptStarted is durable before dispatch. An intent without that event
        // cannot have launched an executor and is safe to supersede.
        let request = NodeExecutionRequest {
            invocation,
            plan_digest: self.journal.run.graph.program_digest.clone(),
            run_directory: self.journal.directory.clone(),
            run_id,
            node: node.clone(),
            attempt,
            workspace: self.runner.workspace.clone(),
            attempt_directory,
            cancellation: self.runner.cancellation.clone(),
            progress,
        };
        let agents = Arc::clone(&self.runner.agents);
        let commands = Arc::clone(&self.runner.commands);
        let custom_providers = Arc::clone(&self.runner.custom_providers);
        self.tasks.spawn(async move {
            rho_providers::provider::scope_custom_openai_compatible_providers(
                custom_providers,
                async move {
                    let cancellation = request.cancellation.clone();
                    let wait_limit_seconds = request.invocation.timeout_seconds;
                    let permit = match gate
                        .acquire(access, &cancellation, wait_limit_seconds)
                        .await
                    {
                        Ok(permit) => permit,
                        Err(RuntimeError::Cancelled) => {
                            return Ok(NodeTaskOutput {
                                node,
                                attempt,
                                result: Ok(NodeExecutionResult::terminal(
                                    NodeTerminalState::Cancellation,
                                )),
                            })
                        }
                        Err(error @ RuntimeError::CheckoutLockTimeout { .. }) => {
                            return Ok(NodeTaskOutput {
                                node,
                                attempt,
                                result: Err(error),
                            })
                        }
                        Err(error) => return Err(error),
                    };
                    let _permit = permit;
                    let (execution, request) = request.split_execution();
                    let result = match execution {
                        PreparedExecution::Agent(invocation) => {
                            agents.execute(request.map_execution(|()| invocation)).await
                        }
                        PreparedExecution::Command(invocation) => {
                            commands
                                .execute(request.map_execution(|()| invocation))
                                .await
                        }
                    };
                    Ok(NodeTaskOutput {
                        node,
                        attempt,
                        result,
                    })
                },
            )
            .await
        });
        Ok(())
    }

    async fn await_next_task(&mut self) -> Result<TaskWait, RuntimeError> {
        let joined = if self.runner.cancellation.is_cancelled() {
            self.tasks.join_next().await
        } else {
            tokio::select! {
                biased;
                joined = self.tasks.join_next() => joined,
                () = self.runner.cancellation.cancelled() => return Ok(TaskWait::Continue),
                () = self.runner.cancel_check.notified() => return Ok(TaskWait::Continue),
                // Fallback for true cross-process cancel request files.
                () = tokio::time::sleep(CROSS_PROCESS_CANCEL_POLL) => return Ok(TaskWait::Continue),
            }
        }
        .ok_or_else(|| RuntimeError::Executor("workflow task set closed".into()))?
        .map_err(|error| RuntimeError::Executor(format!("node task failed: {error}")))??;
        Ok(TaskWait::Joined(Box::new(joined)))
    }

    fn complete_node(&mut self, joined: NodeTaskOutput) -> Result<(), RuntimeError> {
        let NodeTaskOutput {
            node,
            attempt,
            result,
        } = joined;
        let run_id = self.journal.run.manifest.run_id;
        if let Err(RuntimeError::CleanupUncertain { cause }) = &result {
            if *cause == CleanupCause::Cancellation
                && !self.journal.run.state.state.cancellation_requested
            {
                let request_id = read_cancellation_request(&self.journal.store, run_id)?
                    .or_else(|| self.cancellation_request_id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                self.journal
                    .commit(WorkflowEvent::CancellationRequested { request_id })?;
            }
            mark_attempt_uncertain(&self.journal.directory, &node, attempt)?;
            self.journal.commit(WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::NeedsRecovery,
            })?;
            send_event(
                &self.events,
                RuntimeEvent::NeedsRecovery {
                    nodes: vec![node.clone()],
                },
            );
            return Err(RuntimeError::NeedsRecovery {
                nodes: node.to_string(),
            });
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => super::diagnostics::failed_attempt(
                &self.journal,
                &node,
                attempt,
                &error,
                &self.events,
            )?,
        };
        let completion = result.completion(attempt);
        let outcome = completion.outcome;
        let attempt_directory = attempt_directory(&self.journal.directory, &node, attempt);
        write_attempt(
            &self.journal.directory,
            &attempt_directory,
            attempt,
            AttemptState::Completed {
                completion: Box::new(completion.clone()),
            },
        )?;
        if let Some(output) = completion.structured_output.clone() {
            self.journal.commit(WorkflowEvent::StructuredOutput {
                node: node.clone(),
                attempt,
                output,
            })?;
        }
        self.journal.commit(WorkflowEvent::NodeFinished {
            node: node.clone(),
            completion: Box::new(completion.clone()),
        })?;
        if let Some(hooks) = &self.runner.hooks {
            let artifacts = completion.artifacts.references();
            self.journal.commit(WorkflowEvent::HookObserved {
                event: "workflow_node_finished".into(),
                node: Some(node.clone()),
                attempt: Some(attempt),
            })?;
            hooks.observe_workflow_node_finished(crate::hooks::WorkflowNodeFinished {
                workflow_run_id: &run_id.to_string(),
                plan_digest: &self.journal.run.manifest.program_digest.0,
                node_id: &node.to_string(),
                attempt: attempt.get(),
                outcome: &outcome,
                duration: self
                    .attempt_started_at
                    .remove(&node)
                    .map(|started| started.elapsed())
                    .unwrap_or_default(),
                artifacts: &artifacts,
            });
        }
        self.journal.notify(&self.events);
        send_event(&self.events, RuntimeEvent::NodeFinished { node, outcome });
        Ok(())
    }

    fn finish(mut self) -> Result<StoredRun, RuntimeError> {
        if self.journal.run.state.state.cancellation_requested {
            self.journal
                .commit(WorkflowEvent::CancellationAcknowledged {
                    request_id: self.cancellation_request_id.clone().ok_or_else(|| {
                        RuntimeError::Data("cancellation has no durable request identifier".into())
                    })?,
                })?;
        }
        if self.journal.run.state.state.run_result().is_none() {
            return Err(RuntimeError::Data(
                "scheduler stopped before root scope finished".into(),
            ));
        }
        self.journal.commit(WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Completed,
        })?;
        observe_workflow_completion(
            &self.runner.hooks,
            &mut self.journal,
            self.drive_started_at.elapsed(),
        )?;
        send_event(&self.events, RuntimeEvent::Completed);
        Ok(self.journal.run)
    }
}

fn observe_workflow_completion(
    hooks: &Option<Arc<crate::hooks::HookEngine>>,
    journal: &mut RunJournal,
    duration: std::time::Duration,
) -> Result<(), RuntimeError> {
    let Some(hooks) = hooks else {
        return Ok(());
    };
    let outcome = journal
        .run
        .state
        .state
        .outcome()
        .ok_or_else(|| RuntimeError::Data("completed workflow has no outcome".into()))?;
    let event = match outcome {
        crate::workflow::WorkflowOutcome::Success => "workflow_completed",
        crate::workflow::WorkflowOutcome::Cancellation => "workflow_cancelled",
        crate::workflow::WorkflowOutcome::Failure
        | crate::workflow::WorkflowOutcome::Denial
        | crate::workflow::WorkflowOutcome::Blocked => "workflow_failed",
    };
    journal.commit(WorkflowEvent::HookObserved {
        event: event.into(),
        node: None,
        attempt: None,
    })?;
    let run_id = journal.run.manifest.run_id.to_string();
    let digest = &journal.run.manifest.program_digest.0;
    match outcome {
        crate::workflow::WorkflowOutcome::Success => {
            hooks.observe_workflow_completed(&run_id, digest, duration, &[])
        }
        crate::workflow::WorkflowOutcome::Cancellation => {
            hooks.observe_workflow_cancelled(&run_id, digest, duration, &[])
        }
        crate::workflow::WorkflowOutcome::Failure
        | crate::workflow::WorkflowOutcome::Denial
        | crate::workflow::WorkflowOutcome::Blocked => {
            hooks.observe_workflow_failed(&run_id, digest, &outcome, duration, &[])
        }
    }
    Ok(())
}

fn available_capacity(graph: &crate::workflow::FrozenWorkflow) -> SchedulerCapacity {
    SchedulerCapacity {
        total: graph.scheduler.max_parallel_nodes,
        agents: graph.scheduler.max_parallel_agents,
        commands: graph.scheduler.max_parallel_commands,
    }
}

fn write_attempt(
    run_directory: &std::path::Path,
    attempt_directory: &std::path::Path,
    attempt: AttemptNumber,
    state: AttemptState,
) -> Result<(), RuntimeError> {
    write_json(
        run_directory,
        &attempt_directory.join("status.json"),
        &AttemptRecord {
            schema_version: ATTEMPT_VERSION,
            attempt,
            state,
        },
    )
    .map(|_| ())
}
