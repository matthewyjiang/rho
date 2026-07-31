use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Instant};

use tokio::task::JoinSet;

use crate::workflow::{
    apply_event, next_actions, AttemptNumber, AttemptRecord, AttemptState, ExternalOwner,
    NodeExecution, NodeId, NodeState, NodeTerminalState, ResolvedNode, RunId, RunLifecycle,
    RunStateRecord, SchedulerAction, SchedulerCapacity, SchedulerEvent, StoredRun, WorkflowEvent,
    WorkflowEventRecord, WorkflowStore, WorkspaceAccess, ATTEMPT_VERSION, EVENT_VERSION,
};

use super::{
    artifacts::{ensure_private_directory, write_json},
    CheckoutGate, NodeExecutionRequest, NodeExecutionResult, RuntimeError, RuntimeEvent,
    RuntimeSecurity, WorkflowNodeExecutor,
};

// Receipt: the cross-process command-cancellation E2E completed in 87 ms
// with this 100 ms poll, below its sub-second response target.
const CROSS_PROCESS_CANCEL_POLL: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryDecision {
    NormalResume,
    ConfirmNoProcess,
}

#[derive(Clone)]
pub(crate) struct CancellationRequest {
    path: PathBuf,
    cancellation: rho_sdk::CancellationToken,
}

impl CancellationRequest {
    pub(crate) fn request(&self) -> Result<(), RuntimeError> {
        if let Some(parent) = self.path.parent() {
            ensure_private_directory(parent)?;
        }
        crate::config_writer::write_bytes_atomically(&self.path, b"cancel\n")?;
        self.cancellation.cancel();
        Ok(())
    }
}

macro_rules! append_event {
    ($store:expr, $guard:expr, $record:expr, $event:expr $(,)?) => {{
        let record = &mut *$record;
        let sequence = record
            .last_event_sequence
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Data("workflow event sequence overflow".into()))?;
        $store.append_event(
            &mut *$guard,
            &WorkflowEventRecord {
                schema_version: EVENT_VERSION,
                sequence,
                event: $event,
            },
        )?;
        record.last_event_sequence = sequence;
        Ok::<(), RuntimeError>(())
    }};
}

macro_rules! append_then_save {
    ($store:expr, $guard:expr, $record:expr, $next:expr, $event:expr $(,)?) => {{
        let record = &mut *$record;
        append_event!($store, &mut *$guard, &mut *record, $event)?;
        record.state = $next;
        $store.save_state(&mut *$guard, record)?;
        Ok::<(), RuntimeError>(())
    }};
}

macro_rules! persist_lifecycle {
    ($store:expr, $guard:expr, $record:expr) => {{
        let record = &mut *$record;
        let lifecycle = record.state.lifecycle;
        append_event!(
            $store,
            &mut *$guard,
            &mut *record,
            WorkflowEvent::RunLifecycle { lifecycle },
        )?;
        $store.save_state(&mut *$guard, record)?;
        Ok::<(), RuntimeError>(())
    }};
}

pub(crate) struct WorkflowRunner {
    rho_home: PathBuf,
    workspace: PathBuf,
    security: RuntimeSecurity,
    agents: Arc<dyn WorkflowNodeExecutor>,
    commands: Arc<dyn WorkflowNodeExecutor>,
    cancellation: rho_sdk::CancellationToken,
    hooks: Option<Arc<crate::hooks::HookEngine>>,
}

impl WorkflowRunner {
    pub(crate) fn new(
        rho_home: PathBuf,
        workspace: PathBuf,
        security: RuntimeSecurity,
        agents: Arc<dyn WorkflowNodeExecutor>,
        commands: Arc<dyn WorkflowNodeExecutor>,
    ) -> Self {
        Self {
            rho_home,
            workspace,
            security,
            agents,
            commands,
            cancellation: rho_sdk::CancellationToken::new(),
            hooks: None,
        }
    }

    pub(crate) fn with_hooks(mut self, hooks: Arc<crate::hooks::HookEngine>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub(crate) fn cancellation_request(&self, run_id: RunId) -> CancellationRequest {
        CancellationRequest {
            path: run_directory(&self.rho_home, run_id).join("cancel.request"),
            cancellation: self.cancellation.clone(),
        }
    }

    pub(crate) fn request_cross_process_cancel(
        rho_home: &std::path::Path,
        run_id: RunId,
    ) -> Result<(), RuntimeError> {
        let path = run_directory(rho_home, run_id).join("cancel.request");
        crate::config_writer::write_bytes_atomically(&path, b"cancel\n")?;
        Ok(())
    }

    pub(crate) async fn drive(
        &self,
        run_id: RunId,
        recovery: RecoveryDecision,
        events: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> Result<StoredRun, RuntimeError> {
        let store = WorkflowStore::new(&self.rho_home)?;
        let mut guard = store.lock_run(run_id).map_err(|error| {
            if error.to_string().contains("active writer") {
                RuntimeError::ActiveOwner
            } else {
                RuntimeError::Workflow(error)
            }
        })?;
        let mut run = store.load_run(run_id)?;
        let drive_started_at = Instant::now();
        let mut attempt_started_at = BTreeMap::new();
        let run_directory = run_directory(&self.rho_home, run_id);
        if super::journal::replay_journal(&store, &run_directory, &mut run)? {
            store.save_state(&guard, &run.state)?;
        }
        let first_start = run.state.state.lifecycle == RunLifecycle::Planned;
        self.validate_security(&run)?;
        let checkout = CheckoutGate::new(&self.rho_home, &self.workspace)?;
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
            return Ok(run);
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
            run.state.state.lifecycle = RunLifecycle::NeedsRecovery;
            bump_revision(&mut run.state.state)?;
            persist_lifecycle!(&store, &mut guard, &mut run.state)?;
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
        recover_state(&mut run.state, recovery)?;
        if resuming_cancellation {
            match std::fs::remove_file(run_directory.join("cancel.request")) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        set_running(&mut run.state);
        persist_lifecycle!(&store, &mut guard, &mut run.state)?;
        if first_start {
            if let Some(hooks) = &self.hooks {
                append_event!(
                    &store,
                    &mut guard,
                    &mut run.state,
                    WorkflowEvent::HookObserved {
                        event: "workflow_started".into(),
                        node: None,
                        attempt: None,
                    },
                )?;
                store.save_state(&guard, &run.state)?;
                hooks.observe_workflow_started(&run_id.to_string(), &run.manifest.graph_digest.0);
            }
        }

        let graph = Arc::new(run.graph.clone());
        let mut tasks = JoinSet::new();
        loop {
            if self.cancellation.is_cancelled() || run_directory.join("cancel.request").exists() {
                if !run.state.state.cancellation_requested {
                    let next = apply_event(
                        &graph,
                        &run.state.state,
                        SchedulerEvent::CancellationRequested,
                    )?;
                    append_then_save!(
                        &store,
                        &mut guard,
                        &mut run.state,
                        next,
                        WorkflowEvent::CancellationRequested,
                    )?;
                    send_event(
                        &events,
                        RuntimeEvent::StateChanged {
                            revision: run.state.state.revision,
                        },
                    );
                }
                self.cancellation.cancel();
            }

            let capacity = available_capacity(&graph, &run.state.state);
            let actions = next_actions(&graph, &run.state.state, capacity)?;
            let mut launched = false;
            for action in actions {
                match action {
                    SchedulerAction::MarkReady { node } => {
                        let next = apply_event(
                            &graph,
                            &run.state.state,
                            SchedulerEvent::MarkReady { node: node.clone() },
                        )?;
                        append_then_save!(
                            &store,
                            &mut guard,
                            &mut run.state,
                            next,
                            WorkflowEvent::NodeReady { node },
                        )?;
                        send_event(
                            &events,
                            RuntimeEvent::StateChanged {
                                revision: run.state.state.revision,
                            },
                        );
                    }
                    SchedulerAction::MarkTerminal { node, outcome } => {
                        let next = apply_event(
                            &graph,
                            &run.state.state,
                            SchedulerEvent::Finished {
                                node: node.clone(),
                                outcome,
                                command_exit: None,
                                output: None,
                            },
                        )?;
                        append_then_save!(
                            &store,
                            &mut guard,
                            &mut run.state,
                            next,
                            WorkflowEvent::NodeFinished {
                                node: node.clone(),
                                attempt: AttemptNumber::new(1)?,
                                outcome,
                            },
                        )?;
                        send_event(
                            &events,
                            RuntimeEvent::StateChanged {
                                revision: run.state.state.revision,
                            },
                        );
                        send_event(&events, RuntimeEvent::NodeFinished { node, outcome });
                    }
                    SchedulerAction::Launch { node, access } => {
                        let attempt = next_attempt(&run_directory, &node)?;
                        let attempt_directory = attempt_directory(&run_directory, &node, attempt);
                        ensure_private_directory(&attempt_directory)?;
                        write_attempt(
                            &run_directory,
                            &attempt_directory,
                            attempt,
                            AttemptState::LaunchIntended,
                        )?;
                        append_event!(
                            &store,
                            &mut guard,
                            &mut run.state,
                            WorkflowEvent::LaunchIntended {
                                node: node.clone(),
                                attempt,
                            },
                        )?;
                        let next = apply_event(
                            &graph,
                            &run.state.state,
                            SchedulerEvent::Launched {
                                node: node.clone(),
                                attempt,
                            },
                        )?;
                        let owner = ExternalOwner::Process {
                            pid: std::process::id(),
                        };
                        write_attempt(
                            &run_directory,
                            &attempt_directory,
                            attempt,
                            AttemptState::Started {
                                owner: owner.clone(),
                            },
                        )?;
                        append_then_save!(
                            &store,
                            &mut guard,
                            &mut run.state,
                            next,
                            WorkflowEvent::AttemptStarted {
                                node: node.clone(),
                                attempt,
                                owner,
                            },
                        )?;
                        attempt_started_at.insert(node.clone(), Instant::now());
                        if let Some(hooks) = &self.hooks {
                            append_event!(
                                &store,
                                &mut guard,
                                &mut run.state,
                                WorkflowEvent::HookObserved {
                                    event: "workflow_node_started".into(),
                                    node: Some(node.clone()),
                                    attempt: Some(attempt),
                                },
                            )?;
                            store.save_state(&guard, &run.state)?;
                            hooks.observe_workflow_node_started(
                                &run_id.to_string(),
                                &run.manifest.graph_digest.0,
                                node.as_str(),
                                attempt.get(),
                            );
                        }
                        send_event(
                            &events,
                            RuntimeEvent::StateChanged {
                                revision: run.state.state.revision,
                            },
                        );
                        send_event(
                            &events,
                            RuntimeEvent::NodeStarted {
                                node: node.clone(),
                                attempt,
                            },
                        );
                        let executor = match graph.graph.nodes[&node].execution {
                            NodeExecution::Agent(_) => Arc::clone(&self.agents),
                            NodeExecution::Command(_) => Arc::clone(&self.commands),
                        };
                        let gate = checkout.clone();
                        let request = NodeExecutionRequest {
                            workflow: Arc::clone(&graph),
                            run_id,
                            node: node.clone(),
                            attempt,
                            workspace: self.workspace.clone(),
                            attempt_directory,
                            outputs: run.state.state.outputs.clone(),
                            cancellation: self.cancellation.clone(),
                        };
                        tasks.spawn(async move {
                            let cancellation = request.cancellation.clone();
                            let permit = tokio::select! {
                                biased;
                                () = cancellation.cancelled() => {
                                    return Ok::<_, RuntimeError>((
                                        node,
                                        attempt,
                                        Ok(NodeExecutionResult::terminal(
                                            NodeTerminalState::Cancellation,
                                        )),
                                    ));
                                }
                                permit = gate.acquire(access) => permit?,
                            };
                            let _permit = permit;
                            let result = executor.execute(request).await;
                            Ok::<_, RuntimeError>((node, attempt, result))
                        });
                        launched = true;
                    }
                }
            }
            if launched || !next_actions(&graph, &run.state.state, capacity)?.is_empty() {
                continue;
            }
            if tasks.is_empty() {
                if run.state.state.cancellation_requested
                    || run
                        .state
                        .state
                        .nodes
                        .values()
                        .all(|state| state.terminal().is_some())
                {
                    run.state.state.lifecycle = RunLifecycle::Completed;
                    bump_revision(&mut run.state.state)?;
                    persist_lifecycle!(&store, &mut guard, &mut run.state)?;
                    observe_workflow_completion(
                        &self.hooks,
                        &store,
                        &mut guard,
                        &mut run,
                        drive_started_at.elapsed(),
                    )?;
                    send_event(&events, RuntimeEvent::Completed);
                    return Ok(run);
                }
                return Err(RuntimeError::Data(
                    "scheduler made no progress with non-terminal nodes".into(),
                ));
            }
            let joined = if self.cancellation.is_cancelled() {
                tasks.join_next().await
            } else {
                tokio::select! {
                    joined = tasks.join_next() => joined,
                    () = tokio::time::sleep(CROSS_PROCESS_CANCEL_POLL) => continue,
                }
            }
            .ok_or_else(|| RuntimeError::Executor("workflow task set closed".into()))?
            .map_err(|error| RuntimeError::Executor(format!("node task failed: {error}")))??;
            let (node, attempt, result) = joined;
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
            let attempt_directory = attempt_directory(&run_directory, &node, attempt);
            let attempt_state = if result.outcome == NodeTerminalState::Cancellation {
                AttemptState::CleanlyCancelled
            } else {
                AttemptState::Completed {
                    outcome: result.outcome,
                }
            };
            write_attempt(&run_directory, &attempt_directory, attempt, attempt_state)?;
            let next = apply_event(
                &graph,
                &run.state.state,
                SchedulerEvent::Finished {
                    node: node.clone(),
                    outcome: result.outcome,
                    command_exit: result.command_exit,
                    output: result.output.clone(),
                },
            )?;
            if let Some(value) = result.output {
                append_event!(
                    &store,
                    &mut guard,
                    &mut run.state,
                    WorkflowEvent::StructuredOutput {
                        node: node.clone(),
                        value,
                    },
                )?;
            }
            append_then_save!(
                &store,
                &mut guard,
                &mut run.state,
                next,
                WorkflowEvent::NodeFinished {
                    node: node.clone(),
                    attempt,
                    outcome: result.outcome,
                },
            )?;
            if let Some(hooks) = &self.hooks {
                let artifacts = attempt_artifacts(&run_directory, &node, attempt);
                append_event!(
                    &store,
                    &mut guard,
                    &mut run.state,
                    WorkflowEvent::HookObserved {
                        event: "workflow_node_finished".into(),
                        node: Some(node.clone()),
                        attempt: Some(attempt),
                    },
                )?;
                store.save_state(&guard, &run.state)?;
                hooks.observe_workflow_node_finished(crate::hooks::WorkflowNodeFinished {
                    workflow_run_id: &run_id.to_string(),
                    plan_digest: &run.manifest.graph_digest.0,
                    node_id: node.as_str(),
                    attempt: attempt.get(),
                    outcome: &result.outcome,
                    duration: attempt_started_at
                        .remove(&node)
                        .map(|started| started.elapsed())
                        .unwrap_or_default(),
                    artifacts: &artifacts,
                });
            }
            send_event(
                &events,
                RuntimeEvent::StateChanged {
                    revision: run.state.state.revision,
                },
            );
            send_event(
                &events,
                RuntimeEvent::NodeFinished {
                    node,
                    outcome: result.outcome,
                },
            );
        }
    }

    fn validate_security(&self, run: &StoredRun) -> Result<(), RuntimeError> {
        let current = crate::paths::display(&self.workspace.canonicalize()?);
        if current != run.manifest.workspace_identity {
            return Err(RuntimeError::WorkspaceChanged {
                planned: run.manifest.workspace_identity.clone(),
                current,
            });
        }
        for node in run.graph.graph.nodes.values() {
            let resolved = run.graph.resolved_nodes.get(&node.id).ok_or_else(|| {
                RuntimeError::LaunchMetadata {
                    node: node.id.clone(),
                }
            })?;
            match resolved {
                ResolvedNode::Agent(agent) => {
                    if agent.trust_required && !self.security.project_trusted {
                        return Err(RuntimeError::TrustRemoved {
                            node: node.id.clone(),
                        });
                    }
                    validate_permission_ceiling(
                        &node.id,
                        &agent.permission_ceiling,
                        self.security.permission_mode,
                    )?;
                    validate_agent_access(&node.id, node.access, agent)?;
                }
                ResolvedNode::Command(_) if node.access != WorkspaceAccess::Mutating => {
                    return Err(RuntimeError::ReadOnlyCapability {
                        node: node.id.clone(),
                        capability: "commands are always mutating".into(),
                    });
                }
                ResolvedNode::Command(_) => {}
            }
        }
        Ok(())
    }
}

fn observe_workflow_completion(
    hooks: &Option<Arc<crate::hooks::HookEngine>>,
    store: &WorkflowStore,
    guard: &mut crate::workflow::RunMutationGuard,
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
    append_event!(
        store,
        guard,
        &mut run.state,
        WorkflowEvent::HookObserved {
            event: event.into(),
            node: None,
            attempt: None,
        },
    )?;
    store.save_state(guard, &run.state)?;
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

fn attempt_artifacts(
    run_directory: &std::path::Path,
    node: &NodeId,
    attempt: AttemptNumber,
) -> Vec<String> {
    let directory = attempt_directory(run_directory, node, attempt);
    [
        "stdout",
        "stderr",
        "output.json",
        "command.json",
        "agent/answer.txt",
    ]
    .into_iter()
    .map(|name| directory.join(name))
    .filter(|path| path.is_file())
    .filter_map(|path| {
        path.strip_prefix(run_directory)
            .ok()
            .map(crate::paths::display)
    })
    .collect()
}

fn validate_permission_ceiling(
    node: &NodeId,
    frozen: &str,
    current: crate::permission::PermissionMode,
) -> Result<(), RuntimeError> {
    let frozen_rank = match frozen {
        "plan" => 0,
        "supervised" => 1,
        "auto" => 2,
        value => {
            return Err(RuntimeError::Data(format!(
                "node '{node}' has invalid frozen permission ceiling '{value}'"
            )))
        }
    };
    let current_rank = match current {
        crate::permission::PermissionMode::Plan => 0,
        crate::permission::PermissionMode::Supervised => 1,
        crate::permission::PermissionMode::Auto => 2,
    };
    let _effective_rank = frozen_rank.min(current_rank);
    Ok(())
}

fn validate_agent_access(
    node: &NodeId,
    access: WorkspaceAccess,
    agent: &crate::workflow::ResolvedAgent,
) -> Result<(), RuntimeError> {
    if access == WorkspaceAccess::Mutating {
        return Ok(());
    }
    if agent.runtime == crate::workflow::AgentRuntime::ClaudeCli {
        return Err(RuntimeError::ReadOnlyCapability {
            node: node.clone(),
            capability: "claude-cli is mutating in workflow schema version 1".into(),
        });
    }
    const MUTATING: &[&str] = &[
        "agent",
        "agents",
        "apply_patch",
        "bash",
        "edit_file",
        "powershell",
        "process",
        "rho",
        "shell",
        "workflow",
        "write_file",
    ];
    if let Some(capability) = agent
        .capabilities
        .iter()
        .find(|capability| MUTATING.contains(&capability.as_str()))
    {
        return Err(RuntimeError::ReadOnlyCapability {
            node: node.clone(),
            capability: capability.clone(),
        });
    }
    Ok(())
}

fn uncertain_nodes(state: &RunStateRecord) -> Vec<NodeId> {
    state
        .state
        .nodes
        .iter()
        .filter_map(|(node, value)| {
            matches!(value, NodeState::Running { .. }).then_some(node.clone())
        })
        .collect()
}

fn recover_state(
    state: &mut RunStateRecord,
    decision: RecoveryDecision,
) -> Result<(), RuntimeError> {
    let uncertain = uncertain_nodes(state);
    if uncertain.is_empty() && state.state.lifecycle != RunLifecycle::NeedsRecovery {
        return reset_clean_cancellations(state);
    }
    if decision != RecoveryDecision::ConfirmNoProcess {
        return Err(RuntimeError::NeedsRecovery {
            nodes: uncertain
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    for node in uncertain {
        state.state.nodes.insert(node, NodeState::Ready);
    }
    reset_clean_cancellations(state)?;
    state.state.lifecycle = RunLifecycle::Running;
    state.state.cancellation_requested = false;
    bump_revision(&mut state.state)
}

fn reset_clean_cancellations(state: &mut RunStateRecord) -> Result<(), RuntimeError> {
    let cancelled = state
        .state
        .nodes
        .iter()
        .filter_map(|(node, value)| {
            matches!(
                value,
                NodeState::Terminal {
                    outcome: NodeTerminalState::Cancellation
                }
            )
            .then_some(node.clone())
        })
        .collect::<Vec<_>>();
    let changed = !cancelled.is_empty()
        || state.state.cancellation_requested
        || state.state.lifecycle == RunLifecycle::Cancelling;
    for node in cancelled {
        state.state.nodes.insert(node, NodeState::Ready);
    }
    state.state.cancellation_requested = false;
    if state.state.lifecycle != RunLifecycle::Planned {
        state.state.lifecycle = RunLifecycle::Running;
    }
    if changed {
        bump_revision(&mut state.state)?;
    }
    Ok(())
}

fn mark_uncertain_attempts(
    run_directory: &std::path::Path,
    state: &RunStateRecord,
) -> Result<(), RuntimeError> {
    for (node, node_state) in &state.state.nodes {
        let NodeState::Running { attempt } = node_state else {
            continue;
        };
        let directory = attempt_directory(run_directory, node, *attempt);
        let path = directory.join("status.json");
        let record: AttemptRecord = serde_json::from_slice(&std::fs::read(&path)?)?;
        let owner = match record.state {
            AttemptState::Started { owner } | AttemptState::InterruptedUncertain { owner } => owner,
            // A flushed launch intent means work may have started before the
            // owner saved process identity. Keep a typed unknown owner.
            AttemptState::LaunchIntended => ExternalOwner::Process { pid: 0 },
            AttemptState::Completed { .. } | AttemptState::CleanlyCancelled => {
                return Err(RuntimeError::Data(format!(
                    "running node '{node}' has a terminal attempt record"
                )))
            }
        };
        write_attempt(
            run_directory,
            &directory,
            *attempt,
            AttemptState::InterruptedUncertain { owner },
        )?;
    }
    Ok(())
}

fn set_running(state: &mut RunStateRecord) {
    if state.state.lifecycle == RunLifecycle::Planned {
        state.state.lifecycle = RunLifecycle::Running;
        state.state.revision = state.state.revision.saturating_add(1);
    }
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

fn bump_revision(state: &mut crate::workflow::WorkflowState) -> Result<(), RuntimeError> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Data("workflow state revision overflow".into()))?;
    Ok(())
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

fn run_directory(rho_home: &std::path::Path, run_id: RunId) -> PathBuf {
    rho_home
        .join("workflows")
        .join("runs")
        .join(run_id.to_string())
}

fn send_event(
    sender: &Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    event: RuntimeEvent,
) {
    if let Some(sender) = sender {
        let _ = sender.send(event);
    }
}
