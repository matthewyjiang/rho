use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Instant};

use tokio::task::JoinSet;

use crate::workflow::{
    next_actions, AttemptNumber, AttemptRecord, AttemptState, ExternalOwner, NodeExecution, NodeId,
    NodeState, NodeTerminalState, RunId, RunLifecycle, RunMutationGuard, SchedulerAction,
    SchedulerCapacity, StoredRun, WorkflowEvent, WorkflowStore, WorkspaceAccess, ATTEMPT_VERSION,
};

use super::{
    artifacts::write_json,
    cancellation::{
        cancel_waiting_nodes, latest_cancellation_request, latest_pending_cancellation_request,
        read_cancellation_request, run_directory, CROSS_PROCESS_CANCEL_POLL,
    },
    recovery::{mark_attempt_uncertain, mark_uncertain_attempts, recover_state, uncertain_nodes},
    runner::{
        append_event_and_save, append_event_only, persist_state_event,
        recover_completed_transitions, send_event, RecoveryDecision, WorkflowRunner,
    },
    CheckoutGate, CleanupCause, NodeExecutionRequest, NodeExecutionResult, NodeProgressReporter,
    RuntimeError, RuntimeEvent,
};

struct NodeTaskOutput {
    node: NodeId,
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
    store: WorkflowStore,
    guard: RunMutationGuard,
    run: StoredRun,
    drive_started_at: Instant,
    attempt_started_at: BTreeMap<NodeId, Instant>,
    run_directory: PathBuf,
    events: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    cancellation_request_id: Option<String>,
    graph: Arc<crate::workflow::FrozenWorkflow>,
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
        let mut run = store.load_run(run_id)?;
        let drive_started_at = Instant::now();
        let run_directory = run_directory(&runner.rho_home, run_id);
        if super::journal::replay_journal(&store, &run_directory, &mut run)? {
            store.save_state(&mut guard, &run.state)?;
        }
        recover_completed_transitions(&store, &mut guard, &run_directory, &mut run)?;
        let first_start = run.state.state.lifecycle == RunLifecycle::Planned;
        runner.validate_security(&run)?;
        let checkout = CheckoutGate::new(&runner.rho_home, &runner.workspace)?;
        if run.state.state.lifecycle == RunLifecycle::Completed
            && !run.state.state.nodes.values().any(|state| {
                matches!(
                    state,
                    NodeState::Terminal {
                        outcome: NodeTerminalState::Cancellation
                    }
                )
            })
        {
            return Ok(DriveStart::Finished(Box::new(run)));
        }
        let resuming_cancellation = run.state.state.cancellation_requested
            || run.state.state.lifecycle == RunLifecycle::Cancelling
            || run.state.state.nodes.values().any(|state| {
                matches!(
                    state,
                    NodeState::Terminal {
                        outcome: NodeTerminalState::Cancellation
                    }
                )
            });
        let uncertain = uncertain_nodes(&run.state);
        if !uncertain.is_empty() {
            mark_uncertain_attempts(&run_directory, &run.state)?;
            persist_state_event(
                &store,
                &mut guard,
                &run_directory,
                &run.graph,
                &mut run.state,
                WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::NeedsRecovery,
                },
            )?;
            send_event(
                &events,
                RuntimeEvent::NeedsRecovery {
                    nodes: uncertain.clone(),
                },
            );
            if recovery != RecoveryDecision::ConfirmNoProcess {
                return Err(RuntimeError::NeedsRecovery {
                    nodes: uncertain
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            }
        }
        let events_before_recovery = store.read_events(run_id)?;
        let cancellation_request_id = latest_cancellation_request(&events_before_recovery);
        let pending_cancellation_request =
            latest_pending_cancellation_request(&events_before_recovery);
        if run.state.state.cancellation_requested
            && (uncertain.is_empty() || recovery == RecoveryDecision::ConfirmNoProcess)
        {
            if let Some(request_id) = pending_cancellation_request {
                append_event_and_save(
                    &store,
                    &mut guard,
                    &mut run.state,
                    WorkflowEvent::CancellationAcknowledged { request_id },
                )?;
            }
        }
        recover_state(
            &store,
            &mut guard,
            &run_directory,
            &run.graph,
            &mut run.state,
            recovery,
        )?;
        if resuming_cancellation {
            store.clear_cancellation_request(run_id)?;
        }
        if run.state.state.lifecycle != RunLifecycle::Running {
            persist_state_event(
                &store,
                &mut guard,
                &run_directory,
                &run.graph,
                &mut run.state,
                WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::Running,
                },
            )?;
        }
        if first_start {
            if let Some(hooks) = &runner.hooks {
                append_event_and_save(
                    &store,
                    &mut guard,
                    &mut run.state,
                    WorkflowEvent::HookObserved {
                        event: "workflow_started".into(),
                        node: None,
                        attempt: None,
                    },
                )?;
                hooks.observe_workflow_started(&run_id.to_string(), &run.manifest.graph_digest.0);
            }
        }

        let graph = Arc::new(run.graph.clone());
        Ok(DriveStart::Running(Box::new(Self {
            runner,
            store,
            guard,
            run,
            drive_started_at,
            attempt_started_at: BTreeMap::new(),
            run_directory,
            events,
            cancellation_request_id,
            graph,
            checkout,
            tasks: JoinSet::new(),
        })))
    }

    async fn run_loop(mut self) -> Result<StoredRun, RuntimeError> {
        loop {
            self.handle_cancellation_edge()?;

            let capacity = available_capacity(&self.graph, &self.run.state.state);
            let launched = self.handle_scheduler_actions(capacity)?;
            if launched || !next_actions(&self.graph, &self.run.state.state, capacity)?.is_empty() {
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
        let run_id = self.run.manifest.run_id;
        let durable_cancellation = read_cancellation_request(&self.store, run_id)?;
        if self.runner.cancellation.is_cancelled() || durable_cancellation.is_some() {
            if !self.run.state.state.cancellation_requested {
                let request_id =
                    durable_cancellation.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                self.cancellation_request_id = Some(request_id.clone());
                persist_state_event(
                    &self.store,
                    &mut self.guard,
                    &self.run_directory,
                    &self.graph,
                    &mut self.run.state,
                    WorkflowEvent::CancellationRequested { request_id },
                )?;
                send_event(
                    &self.events,
                    RuntimeEvent::StateChanged {
                        revision: self.run.state.state.revision,
                    },
                );
            }
            self.runner.cancellation.cancel();
        }
        Ok(())
    }

    fn handle_scheduler_actions(
        &mut self,
        capacity: SchedulerCapacity,
    ) -> Result<bool, RuntimeError> {
        let actions = next_actions(&self.graph, &self.run.state.state, capacity)?;
        let mut launched = false;
        for action in actions {
            match action {
                SchedulerAction::MarkReady { node } => {
                    persist_state_event(
                        &self.store,
                        &mut self.guard,
                        &self.run_directory,
                        &self.graph,
                        &mut self.run.state,
                        WorkflowEvent::NodeReady { node },
                    )?;
                    send_event(
                        &self.events,
                        RuntimeEvent::StateChanged {
                            revision: self.run.state.state.revision,
                        },
                    );
                }
                SchedulerAction::MarkTerminal { node, outcome } => {
                    persist_state_event(
                        &self.store,
                        &mut self.guard,
                        &self.run_directory,
                        &self.graph,
                        &mut self.run.state,
                        WorkflowEvent::NodeFinished {
                            node: node.clone(),
                            completion: Box::new(crate::workflow::NodeCompletion::terminal(
                                outcome,
                            )),
                        },
                    )?;
                    send_event(
                        &self.events,
                        RuntimeEvent::StateChanged {
                            revision: self.run.state.state.revision,
                        },
                    );
                    send_event(&self.events, RuntimeEvent::NodeFinished { node, outcome });
                }
                SchedulerAction::Launch { node, access } => {
                    self.launch_node(node, access)?;
                    launched = true;
                }
            }
        }
        Ok(launched)
    }

    fn launch_node(&mut self, node: NodeId, access: WorkspaceAccess) -> Result<(), RuntimeError> {
        let run_id = self.run.manifest.run_id;
        let attempt = next_attempt(&self.run_directory, &node)?;
        let attempt_directory = attempt_directory(&self.run_directory, &node, attempt);
        let relative_attempt = attempt_directory
            .strip_prefix(&self.run_directory)
            .map_err(|_| RuntimeError::UnsafeArtifact(attempt_directory.clone()))?;
        crate::workflow::ensure_directory_beneath(&self.run_directory, relative_attempt)?;
        write_attempt(
            &self.run_directory,
            &attempt_directory,
            attempt,
            AttemptState::LaunchIntended,
        )?;
        append_event_only(
            &self.store,
            &mut self.guard,
            &mut self.run.state,
            WorkflowEvent::LaunchIntended {
                node: node.clone(),
                attempt,
            },
        )?;
        let owner = ExternalOwner::Process {
            pid: std::process::id(),
        };
        write_attempt(
            &self.run_directory,
            &attempt_directory,
            attempt,
            AttemptState::Started {
                owner: owner.clone(),
            },
        )?;
        persist_state_event(
            &self.store,
            &mut self.guard,
            &self.run_directory,
            &self.graph,
            &mut self.run.state,
            WorkflowEvent::AttemptStarted {
                node: node.clone(),
                attempt,
                owner,
            },
        )?;
        self.attempt_started_at.insert(node.clone(), Instant::now());
        if let Some(hooks) = &self.runner.hooks {
            append_event_and_save(
                &self.store,
                &mut self.guard,
                &mut self.run.state,
                WorkflowEvent::HookObserved {
                    event: "workflow_node_started".into(),
                    node: Some(node.clone()),
                    attempt: Some(attempt),
                },
            )?;
            hooks.observe_workflow_node_started(
                &run_id.to_string(),
                &self.run.manifest.graph_digest.0,
                node.as_str(),
                attempt.get(),
            );
        }
        send_event(
            &self.events,
            RuntimeEvent::StateChanged {
                revision: self.run.state.state.revision,
            },
        );
        send_event(
            &self.events,
            RuntimeEvent::NodeStarted {
                node: node.clone(),
                attempt,
            },
        );
        let executor = match self.graph.graph.nodes[&node].execution {
            NodeExecution::Agent(_) => Arc::clone(&self.runner.agents),
            NodeExecution::Command(_) => Arc::clone(&self.runner.commands),
        };
        let gate = self.checkout.clone();
        let progress = self
            .events
            .as_ref()
            .map(|sender| NodeProgressReporter::new(node.clone(), attempt, sender.clone()));
        let request = NodeExecutionRequest {
            workflow: Arc::clone(&self.graph),
            run_id,
            node: node.clone(),
            attempt,
            workspace: self.runner.workspace.clone(),
            attempt_directory,
            outputs: self.run.state.state.outputs.clone(),
            cancellation: self.runner.cancellation.clone(),
            progress,
        };
        let custom_providers = Arc::clone(&self.runner.custom_providers);
        self.tasks.spawn(async move {
            rho_providers::provider::scope_custom_openai_compatible_providers(
                custom_providers,
                async move {
                    let cancellation = request.cancellation.clone();
                    let wait_limit_seconds = request.workflow.graph.nodes[&node].timeout_seconds;
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
                            });
                        }
                        Err(error @ RuntimeError::CheckoutLockTimeout { .. }) => {
                            return Ok(NodeTaskOutput {
                                node,
                                attempt,
                                result: Err(error),
                            });
                        }
                        Err(error) => return Err(error),
                    };
                    let _permit = permit;
                    let result = executor.execute(request).await;
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
                // In-process cancel must not wait for the cross-process poll tick.
                () = self.runner.cancellation.cancelled() => {
                    return Ok(TaskWait::Continue);
                }
                // Same-process durable cancel writers can wake the loop immediately.
                () = self.runner.cancel_check.notified() => {
                    return Ok(TaskWait::Continue);
                }
                // Fallback for true cross-process cancel request files.
                () = tokio::time::sleep(CROSS_PROCESS_CANCEL_POLL) => {
                    return Ok(TaskWait::Continue);
                }
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
        let run_id = self.run.manifest.run_id;
        if let Err(RuntimeError::CleanupUncertain { cause }) = &result {
            if *cause == CleanupCause::Cancellation && !self.run.state.state.cancellation_requested
            {
                let request_id = read_cancellation_request(&self.store, run_id)?
                    .or_else(|| self.cancellation_request_id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                persist_state_event(
                    &self.store,
                    &mut self.guard,
                    &self.run_directory,
                    &self.graph,
                    &mut self.run.state,
                    WorkflowEvent::CancellationRequested { request_id },
                )?;
            }
            mark_attempt_uncertain(&self.run_directory, &node, attempt)?;
            persist_state_event(
                &self.store,
                &mut self.guard,
                &self.run_directory,
                &self.graph,
                &mut self.run.state,
                WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::NeedsRecovery,
                },
            )?;
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
            Err(RuntimeError::Denied(_)) => {
                NodeExecutionResult::terminal(NodeTerminalState::Denial)
            }
            Err(RuntimeError::Cancelled) => {
                NodeExecutionResult::terminal(NodeTerminalState::Cancellation)
            }
            Err(_) => NodeExecutionResult::terminal(NodeTerminalState::Failure),
        };
        let completion = result.completion(attempt);
        let outcome = completion.outcome;
        let attempt_directory = attempt_directory(&self.run_directory, &node, attempt);
        write_attempt(
            &self.run_directory,
            &attempt_directory,
            attempt,
            AttemptState::Completed {
                completion: Box::new(completion.clone()),
            },
        )?;
        if let Some(output) = completion.structured_output.clone() {
            append_event_only(
                &self.store,
                &mut self.guard,
                &mut self.run.state,
                WorkflowEvent::StructuredOutput {
                    node: node.clone(),
                    attempt,
                    output,
                },
            )?;
        }
        persist_state_event(
            &self.store,
            &mut self.guard,
            &self.run_directory,
            &self.graph,
            &mut self.run.state,
            WorkflowEvent::NodeFinished {
                node: node.clone(),
                completion: Box::new(completion.clone()),
            },
        )?;
        if let Some(hooks) = &self.runner.hooks {
            let artifacts = completion_artifacts(&completion);
            append_event_and_save(
                &self.store,
                &mut self.guard,
                &mut self.run.state,
                WorkflowEvent::HookObserved {
                    event: "workflow_node_finished".into(),
                    node: Some(node.clone()),
                    attempt: Some(attempt),
                },
            )?;
            hooks.observe_workflow_node_finished(crate::hooks::WorkflowNodeFinished {
                workflow_run_id: &run_id.to_string(),
                plan_digest: &self.run.manifest.graph_digest.0,
                node_id: node.as_str(),
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
        send_event(
            &self.events,
            RuntimeEvent::StateChanged {
                revision: self.run.state.state.revision,
            },
        );
        send_event(&self.events, RuntimeEvent::NodeFinished { node, outcome });
        Ok(())
    }

    fn finish(mut self) -> Result<StoredRun, RuntimeError> {
        if self.run.state.state.cancellation_requested {
            cancel_waiting_nodes(
                &self.store,
                &mut self.guard,
                &self.run_directory,
                &self.graph,
                &mut self.run.state,
            )?;
            append_event_and_save(
                &self.store,
                &mut self.guard,
                &mut self.run.state,
                WorkflowEvent::CancellationAcknowledged {
                    request_id: self.cancellation_request_id.clone().ok_or_else(|| {
                        RuntimeError::Data("cancellation has no durable request identifier".into())
                    })?,
                },
            )?;
        }
        if self
            .run
            .state
            .state
            .nodes
            .values()
            .all(|state| state.terminal().is_some())
        {
            persist_state_event(
                &self.store,
                &mut self.guard,
                &self.run_directory,
                &self.graph,
                &mut self.run.state,
                WorkflowEvent::RunLifecycle {
                    lifecycle: RunLifecycle::Completed,
                },
            )?;
            observe_workflow_completion(
                &self.runner.hooks,
                &self.store,
                &mut self.guard,
                &mut self.run,
                self.drive_started_at.elapsed(),
            )?;
            send_event(&self.events, RuntimeEvent::Completed);
            return Ok(self.run);
        }
        Err(RuntimeError::Data(
            "scheduler made no progress with non-terminal nodes".into(),
        ))
    }
}

fn observe_workflow_completion(
    hooks: &Option<Arc<crate::hooks::HookEngine>>,
    store: &WorkflowStore,
    guard: &mut RunMutationGuard,
    run: &mut StoredRun,
    duration: std::time::Duration,
) -> Result<(), RuntimeError> {
    let Some(hooks) = hooks else {
        return Ok(());
    };
    let outcome = crate::workflow::derive_workflow_outcome(&run.graph, &run.state.state)
        .ok_or_else(|| RuntimeError::Data("completed workflow has no outcome".into()))?;
    let event = match outcome {
        crate::workflow::WorkflowOutcome::Success => "workflow_completed",
        crate::workflow::WorkflowOutcome::Cancellation => "workflow_cancelled",
        crate::workflow::WorkflowOutcome::Failure
        | crate::workflow::WorkflowOutcome::Denial
        | crate::workflow::WorkflowOutcome::Blocked => "workflow_failed",
    };
    append_event_and_save(
        store,
        guard,
        &mut run.state,
        WorkflowEvent::HookObserved {
            event: event.into(),
            node: None,
            attempt: None,
        },
    )?;
    let run_id = run.manifest.run_id.to_string();
    let digest = &run.manifest.graph_digest.0;
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

fn completion_artifacts(
    completion: &crate::workflow::NodeCompletion,
) -> Vec<crate::workflow::DurableArtifactReference> {
    completion.artifacts.references()
}

fn available_capacity(
    graph: &crate::workflow::FrozenWorkflow,
    _state: &crate::workflow::WorkflowState,
) -> SchedulerCapacity {
    SchedulerCapacity {
        total: graph.scheduler.max_parallel_nodes,
        agents: graph.scheduler.max_parallel_agents,
        commands: graph.scheduler.max_parallel_commands,
    }
}

fn next_attempt(
    run_directory: &std::path::Path,
    node: &NodeId,
) -> Result<AttemptNumber, RuntimeError> {
    let attempts = run_directory
        .join("nodes")
        .join(node.as_str())
        .join("attempts");
    let mut highest = 0_u32;
    if attempts.is_dir() {
        for entry in std::fs::read_dir(&attempts)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(value) = entry
                    .file_name()
                    .to_str()
                    .and_then(|value| value.parse::<u32>().ok())
                {
                    highest = highest.max(value);
                }
            }
        }
    }
    AttemptNumber::new(
        highest
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Data("attempt number overflow".into()))?,
    )
    .map_err(RuntimeError::from)
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

fn attempt_directory(
    run_directory: &std::path::Path,
    node: &NodeId,
    attempt: AttemptNumber,
) -> PathBuf {
    run_directory
        .join("nodes")
        .join(node.as_str())
        .join("attempts")
        .join(attempt.to_string())
}
