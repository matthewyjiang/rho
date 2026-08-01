//! Owner and watcher adapters that project durable runs into the workflow TUI.

use std::{future::Future, pin::Pin, sync::Arc};

use sha2::{Digest as _, Sha256};

use crate::{
    app::workflow_runtime::{
        RecoveryDecision, RuntimeError, RuntimeEvent, WorkflowRunner,
    },
    tui::workflow::{
        CancellationState, ExecutionMetadata, PlanApprovalState, SourceDigestSummary,
        TerminalReason, WorkflowAction, WorkflowEvent as TuiEvent, WorkflowEventAdapter,
        WorkflowNodeSnapshot, WorkflowProgress, WorkflowSession, WorkflowSnapshot,
    },
    workflow::{
        derive_workflow_outcome, CommandNode, Digest, NodeExecution, NodeState, NodeTerminalState,
        ResolvedNode, RunId, RunLifecycle, StoredRun, Template, TemplatePart, WorkflowStore,
    },
};

/// Poll interval for read-only watch of a durable run snapshot.
const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Opens the workflow DAG screen in read-only watch mode for an existing run.
pub(crate) async fn watch_run(run: StoredRun) -> anyhow::Result<()> {
    let adapter = WatchAdapter::new(crate::paths::rho_dir()?, run)?;
    crate::tui::workflow::run(Box::new(adapter)).await?;
    Ok(())
}

pub(super) struct RunnerTuiAdapter {
    runner: Arc<WorkflowRunner>,
    rho_home: std::path::PathBuf,
    run_id: RunId,
    initial: WorkflowSnapshot,
    events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    worker: Option<tokio::task::JoinHandle<Result<StoredRun, RuntimeError>>>,
}

impl RunnerTuiAdapter {
    pub(super) fn start(
        runner: Arc<WorkflowRunner>,
        rho_home: std::path::PathBuf,
        run: StoredRun,
        recovery: RecoveryDecision,
    ) -> Self {
        let run_id = run.manifest.run_id;
        let initial = tui_snapshot(&run);
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        let worker_runner = Arc::clone(&runner);
        let worker =
            tokio::spawn(async move { worker_runner.drive(run_id, recovery, Some(sender)).await });
        Self {
            runner,
            rho_home,
            run_id,
            initial,
            events,
            worker: Some(worker),
        }
    }

    fn load_snapshot(&self) -> anyhow::Result<WorkflowSnapshot> {
        let store = WorkflowStore::new(&self.rho_home)?;
        Ok(tui_snapshot(&store.load_run(self.run_id)?))
    }

    async fn finish_worker(&mut self) -> anyhow::Result<()> {
        if let Some(worker) = self.worker.take() {
            worker
                .await
                .map_err(|error| anyhow::anyhow!("workflow runner task failed: {error}"))??;
        }
        Ok(())
    }
}

impl WorkflowEventAdapter for RunnerTuiAdapter {
    fn session(&self) -> WorkflowSession {
        WorkflowSession::Owner
    }

    fn initial_snapshot(&self) -> WorkflowSnapshot {
        self.initial.clone()
    }

    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<TuiEvent>>> + Send + '_>> {
        Box::pin(async move {
            match self.events.recv().await {
                Some(RuntimeEvent::NodeProgress {
                    node,
                    attempt,
                    message,
                    detail,
                    completed,
                    total,
                }) => Ok(Some(TuiEvent::Progress {
                    node,
                    progress: WorkflowProgress {
                        attempt,
                        completed,
                        total,
                        message,
                        detail,
                    },
                })),
                Some(_) => self
                    .load_snapshot()
                    .map(|snapshot| Some(TuiEvent::Snapshot(snapshot))),
                None => {
                    self.finish_worker().await?;
                    Ok(None)
                }
            }
        })
    }

    fn send(
        &mut self,
        action: WorkflowAction,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match action {
                WorkflowAction::Cancel => {
                    self.runner.cancellation_request(self.run_id).request()?;
                }
                WorkflowAction::ConfirmPlan | WorkflowAction::ConfirmResume => {
                    anyhow::bail!("the workflow plan was already confirmed")
                }
            }
            Ok(())
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.runner.cancellation_request(self.run_id).request()?;
            self.finish_worker().await
        })
    }
}

struct WatchAdapter {
    store: WorkflowStore,
    rho_home: std::path::PathBuf,
    run_id: RunId,
    initial: WorkflowSnapshot,
    last_revision: u64,
    interval: tokio::time::Interval,
}

impl WatchAdapter {
    fn new(rho_home: std::path::PathBuf, run: StoredRun) -> anyhow::Result<Self> {
        let run_id = run.manifest.run_id;
        let last_revision = run.state.state.revision;
        let mut interval = tokio::time::interval(WATCH_POLL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Ok(Self {
            store: WorkflowStore::new(&rho_home)?,
            rho_home,
            run_id,
            initial: tui_snapshot(&run),
            last_revision,
            interval,
        })
    }

    /// Cheap poll: read revision only. Full load happens on change.
    fn load_if_changed(&self) -> anyhow::Result<Option<(WorkflowSnapshot, u64)>> {
        let revision = self.store.read_run_revision(self.run_id)?;
        if revision == self.last_revision {
            return Ok(None);
        }
        let run = self.store.load_run(self.run_id)?;
        let revision = run.state.state.revision;
        Ok(Some((tui_snapshot(&run), revision)))
    }
}

impl WorkflowEventAdapter for WatchAdapter {
    fn session(&self) -> WorkflowSession {
        WorkflowSession::Watcher
    }

    fn initial_snapshot(&self) -> WorkflowSnapshot {
        self.initial.clone()
    }

    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<TuiEvent>>> + Send + '_>> {
        Box::pin(async move {
            loop {
                self.interval.tick().await;
                if let Some((snapshot, revision)) = self.load_if_changed()? {
                    self.last_revision = revision;
                    return Ok(Some(TuiEvent::Snapshot(snapshot)));
                }
            }
        })
    }

    fn send(
        &mut self,
        action: WorkflowAction,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match action {
                WorkflowAction::Cancel => {
                    let lifecycle = self.store.read_run_lifecycle(self.run_id)?;
                    crate::app::workflow_cli::request_cancellation(
                        &self.rho_home,
                        self.run_id,
                        lifecycle,
                    )
                    .await?;
                }
                WorkflowAction::ConfirmPlan | WorkflowAction::ConfirmResume => {
                    anyhow::bail!("watch mode cannot start or resume a plan")
                }
            }
            Ok(())
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }
}

fn tui_snapshot(run: &StoredRun) -> WorkflowSnapshot {
    let state = &run.state.state;
    let nodes = run
        .graph
        .graph
        .nodes
        .iter()
        .map(|(id, node)| {
            let node_state = state.nodes[id].clone();
            let current_attempt = match node_state {
                NodeState::Running { attempt } => Some(attempt),
                _ => None,
            };
            let execution = match &run.graph.resolved_nodes[id] {
                ResolvedNode::Agent(agent) => ExecutionMetadata::Agent {
                    name: agent.agent_id.clone(),
                    runtime: agent.runtime,
                    provider: agent.provider.clone(),
                    model: agent.model.clone(),
                },
                ResolvedNode::Command(command) => ExecutionMetadata::Command {
                    executable: command.executable.clone(),
                    cwd: command.cwd.clone(),
                    shell: matches!(
                        node.execution,
                        NodeExecution::Command(crate::workflow::CommandNode::Shell { .. })
                    ),
                },
            };
            WorkflowNodeSnapshot {
                id: id.clone(),
                display_name: node.display_name.clone(),
                dependencies: node.needs.clone(),
                access: node.access,
                execution,
                work: node_work_summary(&node.execution),
                state: node_state.clone(),
                current_attempt,
                command_exit: state.command_exits.get(id).cloned(),
                validated_output: state.outputs.get(id).cloned(),
                artifacts: durable_artifacts_for_node(state, id),
                terminal_reason: terminal_reason(&node_state),
            }
        })
        .collect();
    let lifecycle = state.lifecycle;
    WorkflowSnapshot {
        workflow_name: run.graph.graph.name.to_string(),
        plan_id: run.manifest.plan_id,
        run_id: Some(run.manifest.run_id),
        graph_digest: run.manifest.graph_digest.clone(),
        sources: SourceDigestSummary {
            source_count: run.graph.sources.modules.len(),
            digest: source_digest(run),
        },
        approval: PlanApprovalState::Approved,
        lifecycle,
        outcome: derive_workflow_outcome(&run.graph, state),
        nodes,
        cancellation: if state.cancellation_requested {
            if lifecycle == RunLifecycle::Completed {
                CancellationState::Saved
            } else {
                CancellationState::Requested
            }
        } else {
            CancellationState::NotRequested
        },
        recovery_requirement: None,
    }
}

pub(super) fn durable_artifacts_for_node(
    state: &crate::workflow::WorkflowState,
    id: &crate::workflow::NodeId,
) -> Vec<crate::tui::workflow::ArtifactReference> {
    state
        .completions
        .get(id)
        .into_iter()
        .flat_map(|completion| completion.artifacts.iter())
        .map(|(kind, artifact)| crate::tui::workflow::ArtifactReference {
            kind,
            artifact: artifact.clone(),
        })
        .collect()
}

fn node_work_summary(execution: &NodeExecution) -> String {
    match execution {
        NodeExecution::Agent(agent) => {
            let preview = template_preview(&agent.prompt);
            if preview.is_empty() {
                format!("agent {}", agent.agent)
            } else {
                preview
            }
        }
        NodeExecution::Command(CommandNode::Direct {
            executable,
            arguments,
            ..
        }) => {
            let exe = std::path::Path::new(executable)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(executable);
            if arguments.is_empty() {
                format!("run {exe}")
            } else {
                let args = arguments
                    .iter()
                    .map(template_preview)
                    .collect::<Vec<_>>()
                    .join(" ");
                truncate_chars(&format!("run {exe} {args}"), 160)
            }
        }
        NodeExecution::Command(CommandNode::Shell { command, .. }) => {
            truncate_chars(&format!("shell: {command}"), 160)
        }
    }
}

fn template_preview(template: &Template) -> String {
    let mut out = String::new();
    for part in &template.0 {
        match part {
            TemplatePart::Literal { value } => out.push_str(value),
            TemplatePart::Output { reference } => {
                let path = if reference.path.0.is_empty() {
                    String::new()
                } else {
                    format!(".{}", reference.path.0.join("."))
                };
                out.push_str(&format!("{{{{{node}{path}}}}}", node = reference.node));
            }
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, 160)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn source_digest(run: &StoredRun) -> Digest {
    let mut hash = Sha256::new();
    for (label, source) in &run.graph.sources.modules {
        hash.update(label.as_bytes());
        hash.update([0]);
        hash.update(source.digest.0.as_bytes());
        hash.update([0]);
    }
    Digest(format!("sha256:{:x}", hash.finalize()))
}

fn terminal_reason(state: &NodeState) -> Option<TerminalReason> {
    let NodeState::Terminal { outcome } = state else {
        return None;
    };
    match outcome {
        NodeTerminalState::Success | NodeTerminalState::Skipped => None,
        NodeTerminalState::Failure => Some(TerminalReason::Failure("node failed".into())),
        NodeTerminalState::Denial => Some(TerminalReason::Denial("node was denied".into())),
        NodeTerminalState::Cancellation => {
            Some(TerminalReason::Cancellation("node was cancelled".into()))
        }
        NodeTerminalState::Blocked => Some(TerminalReason::Blocked("node was blocked".into())),
    }
}
