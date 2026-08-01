//! Owner and watcher adapters that bridge durable runs into the workflow TUI.
//!
//! Snapshot projection lives in `tui::workflow::snapshot`. This module only
//! owns event delivery, cancel, and worker lifecycle.

use std::{future::Future, pin::Pin, sync::Arc};

use crate::{
    app::workflow_runtime::{RecoveryDecision, RuntimeError, RuntimeEvent, WorkflowRunner},
    tui::workflow::{
        snapshot, WorkflowAction, WorkflowEvent as TuiEvent, WorkflowEventAdapter,
        WorkflowProgress, WorkflowSession, WorkflowSnapshot,
    },
    workflow::{RunId, StoredRun, WorkflowStore},
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
    store: WorkflowStore,
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
    ) -> anyhow::Result<Self> {
        let run_id = run.manifest.run_id;
        let initial = snapshot::from_stored_run(&run);
        let store = WorkflowStore::new(&rho_home)?;
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        let worker_runner = Arc::clone(&runner);
        let worker =
            tokio::spawn(async move { worker_runner.drive(run_id, recovery, Some(sender)).await });
        Ok(Self {
            runner,
            store,
            rho_home,
            run_id,
            initial,
            events,
            worker: Some(worker),
        })
    }

    fn load_snapshot(&self) -> anyhow::Result<WorkflowSnapshot> {
        Ok(snapshot::from_stored_run(
            &self.store.load_run(self.run_id)?,
        ))
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

    fn run_directory(&self) -> Option<std::path::PathBuf> {
        Some(crate::workflow::WorkflowLayout::new(&self.rho_home).run(self.run_id))
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

    fn finish(&mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move { self.finish_worker().await })
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
            initial: snapshot::from_stored_run(&run),
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
        Ok(Some((snapshot::from_stored_run(&run), revision)))
    }
}

impl WorkflowEventAdapter for WatchAdapter {
    fn session(&self) -> WorkflowSession {
        WorkflowSession::Watcher
    }

    fn initial_snapshot(&self) -> WorkflowSnapshot {
        self.initial.clone()
    }

    fn run_directory(&self) -> Option<std::path::PathBuf> {
        Some(crate::workflow::WorkflowLayout::new(&self.rho_home).run(self.run_id))
    }

    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<TuiEvent>>> + Send + '_>> {
        Box::pin(async move {
            loop {
                // Check first so a just-changed revision is not delayed by a full poll tick.
                if let Some((snapshot, revision)) = self.load_if_changed()? {
                    self.last_revision = revision;
                    return Ok(Some(TuiEvent::Snapshot(snapshot)));
                }
                self.interval.tick().await;
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
                    super::request_cancellation(&self.rho_home, self.run_id, lifecycle).await?;
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
