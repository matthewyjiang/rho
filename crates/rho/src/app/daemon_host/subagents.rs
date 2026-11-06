//! Subagent and workflow runs as child jobs (#1658). Every daemon session's
//! subagents, and the agent nodes of the workflow runs it drives, take
//! capacity from one pool, the daemon-wide budget. Each workflow run is a
//! job whose parent is the job of the session that started it, and each
//! subagent or agent node run is a job whose parent is the job of the
//! session or workflow run that started it.
//!
//! The budget's limit is `behavior.agent_concurrency` in the user's config,
//! read again whenever a session opens, so an edit reaches every session's
//! subagents once the next session opened. A queued run's job reports where it
//! stands in the budget's queue.
//!
//! A run's job follows the run's status: `queued` until the run has
//! capacity, `running`, then `completed`, `failed`, or `cancelled`. A run
//! stopped because the daemon is stopping ends `interrupted`, as does one
//! the daemon's stop leaves unfinished (see `JobRegistry`).
//!
//! A workflow run's job is `running` while the session drives the run, then
//! takes how the drive ended. A drive the daemon's stop cuts short ends
//! `interrupted`, and the run resumes like one whose process died.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_sdk::CancellationToken;
use tokio::sync::{oneshot, watch};

use super::jobs::{JobEvent, JobId, JobRecord, JobRegistry, JobState, TurnEnd};
use crate::{
    app::{
        agent_concurrency::{AgentConcurrency, QueuePlace, QueueWatch},
        subagent_pool::{
            ChildObserver, HostedWorkflow, StartedSubagent, StartedWorkflow, SubagentPool,
            WorkflowEnd,
        },
    },
    config::{Config, DEFAULT_AGENT_CONCURRENCY},
    subagent::{RunState, RunStatus},
};

/// The daemon-wide budget and the jobs of the runs it admits.
pub(super) struct Subagents {
    concurrency: AgentConcurrency,
    jobs: Arc<JobRegistry>,
    queues: Queues,
    /// Cancelled once the daemon starts stopping.
    stopping: CancellationToken,
}

/// Where each run waits for capacity, by its job, until the run has it.
type Queues = Arc<Mutex<HashMap<JobId, QueueWatch>>>;

impl Subagents {
    /// No runs yet; their jobs are recorded in `jobs`. The pool starts at
    /// the default limit, which the first session's open replaces.
    pub(super) fn new(jobs: Arc<JobRegistry>, stopping: CancellationToken) -> Self {
        Self {
            concurrency: AgentConcurrency::from_config(DEFAULT_AGENT_CONCURRENCY),
            jobs,
            queues: Queues::default(),
            stopping,
        }
    }

    /// Sets the daemon-wide limit to `config`'s `agent_concurrency`. Runs
    /// already going keep their slots.
    pub(super) fn set_limit(&self, config: &Config) {
        self.concurrency.set_total(config.agent_concurrency);
    }

    /// The pool of the session whose turns are job `parent`: the
    /// daemon-wide one, each run recorded as a child job of `parent`.
    pub(super) fn pool_for(&self, parent: JobId) -> SubagentPool {
        ChildJobs {
            parent,
            concurrency: self.concurrency.clone(),
            jobs: Arc::clone(&self.jobs),
            queues: Arc::clone(&self.queues),
            stopping: self.stopping.clone(),
        }
        .into_pool()
    }

    /// Where job `job`'s run waits for capacity, while it does.
    pub(super) fn queue_place(&self, job: &JobId) -> Option<QueuePlace> {
        lock(&self.queues).get(job)?.place()
    }
}

/// Records the runs a session or workflow run starts as children of its
/// job.
struct ChildJobs {
    parent: JobId,
    concurrency: AgentConcurrency,
    jobs: Arc<JobRegistry>,
    queues: Queues,
    stopping: CancellationToken,
}

impl ChildJobs {
    /// The daemon-wide pool, its runs recorded as children of `self.parent`.
    fn into_pool(self) -> SubagentPool {
        SubagentPool::Shared {
            concurrency: self.concurrency.clone(),
            observer: Arc::new(self),
        }
    }

    /// The same, recording runs as children of `parent` instead.
    fn with_parent(&self, parent: JobId) -> Self {
        Self {
            parent,
            concurrency: self.concurrency.clone(),
            jobs: Arc::clone(&self.jobs),
            queues: Arc::clone(&self.queues),
            stopping: self.stopping.clone(),
        }
    }
}

impl ChildObserver for ChildJobs {
    fn workflow_started(&self, run: StartedWorkflow) -> HostedWorkflow {
        let StartedWorkflow { run_id, workspace } = run;
        let (end, ended) = oneshot::channel();
        let record = JobRecord::new_workflow(self.parent.clone(), workspace, run_id);
        let job = record.id.clone();
        if let Err(error) = self.jobs.register(record) {
            tracing::error!(parent = %self.parent, run = %run_id, %error, "could not record a workflow run's job; the run goes on unlisted, its agent nodes under the session's job");
            return HostedWorkflow {
                nodes: self.with_parent(self.parent.clone()).into_pool(),
                end,
            };
        }
        tracing::info!(parent = %self.parent, %job, run = %run_id, "workflow run started");
        tokio::spawn(follow_workflow(
            job.clone(),
            ended,
            Arc::clone(&self.jobs),
            self.stopping.clone(),
        ));
        HostedWorkflow {
            nodes: self.with_parent(job).into_pool(),
            end,
        }
    }

    fn subagent_started(&self, run: StartedSubagent) {
        let StartedSubagent {
            run_id,
            session,
            workspace,
            status,
            queue,
        } = run;
        let record = JobRecord::new_subagent(self.parent.clone(), workspace, session);
        let job = record.id.clone();
        if let Err(error) = self.jobs.register(record) {
            tracing::error!(parent = %self.parent, run = %run_id, %error, "could not record a subagent run's job; the run goes on unlisted");
            return;
        }
        tracing::info!(parent = %self.parent, %job, run = %run_id, "subagent run started");
        lock(&self.queues).insert(job.clone(), queue);
        tokio::spawn(follow(
            job,
            status,
            Arc::clone(&self.jobs),
            Arc::clone(&self.queues),
            self.stopping.clone(),
        ));
    }
}

/// Moves job `job` along with its run's `status` until the run ends.
async fn follow(
    job: JobId,
    mut status: watch::Receiver<RunStatus>,
    jobs: Arc<JobRegistry>,
    queues: Queues,
    stopping: CancellationToken,
) {
    let mut state = JobState::Queued;
    loop {
        let run = status.borrow_and_update().clone();
        if let Some(event) = next_event(&state, &run, stopping.is_cancelled()) {
            match jobs.record(&job, event) {
                Ok(next) => state = next,
                // Only the daemon's stop moves a subagent job on besides
                // this task, by recording it interrupted, which no later
                // event of the run undoes.
                Err(error) => {
                    tracing::debug!(%job, %error, "subagent job already ended");
                    break;
                }
            }
        }
        if run.state.is_terminal() {
            break;
        }
        if status.changed().await.is_err() {
            // The executor sends a terminal status before it lets go of
            // the run, even when the run panicked, so only a runtime that
            // shuts down under it gets here.
            let end = if stopping.is_cancelled() {
                TurnEnd::Interrupted
            } else {
                TurnEnd::Failed("the subagent run ended without a status".into())
            };
            if let Err(error) = jobs.record(&job, JobEvent::TurnEnded(end)) {
                tracing::debug!(%job, %error, "subagent job already ended");
            }
            break;
        }
    }
    lock(&queues).remove(&job);
}

/// Ends workflow job `job` with how its drive `ended`.
async fn follow_workflow(
    job: JobId,
    ended: oneshot::Receiver<WorkflowEnd>,
    jobs: Arc<JobRegistry>,
    stopping: CancellationToken,
) {
    let end = workflow_end(ended.await.ok(), stopping.is_cancelled());
    if let Err(error) = jobs.record(&job, JobEvent::TurnEnded(end)) {
        // Only the daemon's stop, recording it interrupted, ends it first.
        tracing::debug!(%job, %error, "workflow job already ended");
    }
}

/// How a workflow job ends, given how its drive ended, or `None` when the
/// drive stopped without saying: the daemon's runtime shut down under it,
/// which while the daemon is `stopping` is an interruption.
fn workflow_end(ended: Option<WorkflowEnd>, stopping: bool) -> TurnEnd {
    match ended {
        Some(WorkflowEnd::Completed) => TurnEnd::Completed,
        Some(WorkflowEnd::Cancelled) => TurnEnd::Cancelled,
        Some(WorkflowEnd::Failed(error)) => TurnEnd::Failed(error),
        None if stopping => TurnEnd::Interrupted,
        None => TurnEnd::Failed("the workflow run stopped without an end".into()),
    }
}

/// The event that moves a subagent job in `state` to where its `run` is, or
/// `None` when it is there already. A run stopped while the daemon is
/// `stopping` was interrupted, not cancelled.
fn next_event(state: &JobState, run: &RunStatus, stopping: bool) -> Option<JobEvent> {
    let end = match run.state {
        RunState::Queued => return None,
        RunState::Starting | RunState::Running => {
            return (*state == JobState::Queued).then_some(JobEvent::Admitted);
        }
        RunState::Ok => TurnEnd::Completed,
        RunState::Error => TurnEnd::Failed(
            run.error
                .clone()
                .unwrap_or_else(|| "the subagent run failed".into()),
        ),
        RunState::Stopped if stopping => TurnEnd::Interrupted,
        RunState::Stopped => TurnEnd::Cancelled,
    };
    state.in_progress().then_some(JobEvent::TurnEnded(end))
}

fn lock(queues: &Queues) -> MutexGuard<'_, HashMap<JobId, QueueWatch>> {
    // Entries are only inserted and removed whole.
    queues.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "subagents_tests.rs"]
mod tests;
