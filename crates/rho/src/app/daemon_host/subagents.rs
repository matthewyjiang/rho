//! Subagent and workflow runs as child jobs (#1658). Every daemon session's
//! subagents, and the agent nodes of the workflow runs it drives, take
//! capacity from one pool, the daemon-wide budget. Each workflow run is a
//! job whose parent is the job of the session that started it, and each
//! subagent or agent node run is a job whose parent is the job of the
//! session or workflow run that started it.
//!
//! The budget's limit is `behavior.agent_concurrency` in the user's config,
//! applied as soon as the daemon's settings store publishes a change (#1720).
//! A queued run's job reports where it stands in the budget's queue.
//!
//! A run's job follows the run's status: `queued` until the run has
//! capacity, `running`, then `completed`, `failed`, or `cancelled`. A run
//! stopped because the daemon is stopping ends `interrupted`, as does one
//! the daemon's stop leaves unfinished (see `JobRegistry`).
//!
//! A run on the Rho runtime also streams its session's events (#1668): the
//! task that follows its status writes them to a stream clients observe by
//! the run's session (see `sessions::child_feeds`), with the job's states
//! and how the run ended. Runs on other runtimes keep no Rho session and
//! stream nothing.
//!
//! A workflow run's job is `running` while the session drives the run, then
//! takes how the drive ended. A drive the daemon's stop cuts short ends
//! `interrupted`, and the run resumes like one whose process died.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_protocol::{AnswerStop, DetachReason, TurnError, TurnErrorKind, TurnOutcome};
use rho_sdk::CancellationToken;
use tokio::sync::{oneshot, watch};

use super::{
    jobs::{JobEvent, JobId, JobRecord, JobRegistry, JobState, RegisterError, TurnEnd},
    sessions::{ChildFeeds, ChildStream},
};
use crate::{
    app::{
        agent_binding::CapacityClass,
        agent_concurrency::{AgentConcurrency, QueuePlace, QueueWatch, QueuedRun},
        subagent_pool::{
            ChildEvent, ChildEvents, ChildObserver, HostedWorkflow, StartedSubagent,
            StartedWorkflow, SubagentPool, WorkflowEnd,
        },
    },
    config::DEFAULT_AGENT_CONCURRENCY,
    subagent::{RunState, RunStatus},
};

/// The daemon-wide budget and the jobs of the runs it admits.
pub(super) struct Subagents {
    concurrency: AgentConcurrency,
    jobs: Arc<JobRegistry>,
    queues: Queues,
    /// The event streams of the runs under way.
    feeds: ChildFeeds,
    /// Cancelled once the daemon starts stopping.
    stopping: CancellationToken,
}

/// Where each run waits for capacity, by its job, until the run has it.
type Queues = Arc<Mutex<HashMap<JobId, QueueWatch>>>;

impl Subagents {
    /// No runs yet; their jobs are recorded in `jobs`. The pool starts at
    /// the default limit, which the settings store replaces once it read
    /// the config.
    pub(super) fn new(jobs: Arc<JobRegistry>, stopping: CancellationToken) -> Self {
        Self {
            concurrency: AgentConcurrency::from_config(DEFAULT_AGENT_CONCURRENCY),
            jobs,
            queues: Queues::default(),
            feeds: ChildFeeds::default(),
            stopping,
        }
    }

    /// The event streams of the runs under way, which a client observes.
    pub(super) fn feeds(&self) -> &ChildFeeds {
        &self.feeds
    }

    /// The daemon-wide budget, whose limit the settings store sets from
    /// `behavior.agent_concurrency` (see `settings`). Runs already going
    /// keep their slots when it shrinks.
    pub(super) fn concurrency(&self) -> AgentConcurrency {
        self.concurrency.clone()
    }

    /// The pool of the session whose turns are job `parent`: the
    /// daemon-wide one, each run recorded as a child job of `parent`.
    pub(super) fn pool_for(&self, parent: JobId) -> SubagentPool {
        SubagentPool::Shared {
            concurrency: self.concurrency.clone(),
            observer: Arc::new(ChildJobs {
                parent,
                jobs: Arc::clone(&self.jobs),
                queues: Arc::clone(&self.queues),
                feeds: self.feeds.clone(),
                stopping: self.stopping.clone(),
            }),
        }
    }

    /// Puts a turn of session job `job`, which another session started, in
    /// line for capacity in the daemon-wide budget, which it shares with
    /// subagent runs (#1714). The job reports where it waits for as long as
    /// the turn's [`TurnBudget::listing`] lives.
    pub(super) fn queue_turn(&self, job: &JobId) -> TurnBudget {
        let queued = self.concurrency.join(CapacityClass::Rho);
        lock(&self.queues).insert(job.clone(), queued.watch());
        TurnBudget {
            queued,
            concurrency: self.concurrency.clone(),
            listing: Listing {
                job: job.clone(),
                queues: Arc::clone(&self.queues),
            },
        }
    }

    /// Where job `job`'s run waits for capacity, while it does.
    pub(super) fn queue_place(&self, job: &JobId) -> Option<QueuePlace> {
        lock(&self.queues).get(job)?.place()
    }
}

/// A started session's turn in line for the daemon-wide budget.
pub(super) struct TurnBudget {
    pub(super) queued: QueuedRun,
    /// The budget, for taking a slot again after lending it (see
    /// `sessions::budget`).
    pub(super) concurrency: AgentConcurrency,
    /// Keeps the job's place in line listed until dropped.
    pub(super) listing: Listing,
}

/// A job's place in line, listed for `rho/job/list` until dropped.
pub(super) struct Listing {
    job: JobId,
    queues: Queues,
}

impl Drop for Listing {
    fn drop(&mut self) {
        lock(&self.queues).remove(&self.job);
    }
}

/// Records the runs a session or workflow run starts as children of its
/// job.
#[derive(Clone)]
struct ChildJobs {
    parent: JobId,
    jobs: Arc<JobRegistry>,
    queues: Queues,
    feeds: ChildFeeds,
    stopping: CancellationToken,
}

impl ChildObserver for ChildJobs {
    fn workflow_started(&self, run: StartedWorkflow) -> HostedWorkflow {
        let StartedWorkflow { run_id, workspace } = run;
        let (end, ended) = oneshot::channel();
        let record = JobRecord::new_workflow(self.parent.clone(), workspace, run_id);
        let job = record.id.clone();
        if let Err(error) = self.jobs.register(record) {
            unlisted(&error, &self.parent, &run_id.to_string(), "workflow");
            return HostedWorkflow {
                observer: Arc::new(self.clone()),
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
            observer: Arc::new(Self {
                parent: job,
                ..self.clone()
            }),
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
            events,
        } = run;
        let record = JobRecord::new_subagent(
            self.parent.clone(),
            workspace.clone(),
            session.clone(),
            run_id.clone(),
        );
        let job = record.id.clone();
        // Listed before the job is, so a client that finds the job finds
        // its stream.
        let followed = session.zip(events).map(|(session, events)| Followed {
            events: Some(events),
            stream: self.feeds.list(session, workspace, &record.state),
        });
        if let Err(error) = self.jobs.register(record) {
            unlisted(&error, &self.parent, &run_id, "subagent");
            return;
        }
        tracing::info!(parent = %self.parent, %job, run = %run_id, "subagent run started");
        lock(&self.queues).insert(job.clone(), queue);
        tokio::spawn(follow(
            job,
            status,
            followed,
            Arc::clone(&self.jobs),
            Arc::clone(&self.queues),
            self.stopping.clone(),
        ));
    }
}

/// Logs why the `kind` run `run` that job `parent` started goes on with no
/// job of its own. A workflow run's agent nodes are then listed under
/// `parent`.
fn unlisted(error: &RegisterError, parent: &JobId, run: &str, kind: &str) {
    match error {
        RegisterError::Closed => {
            tracing::info!(%parent, run, kind, "daemon stopping; a run started during the stop goes on unlisted");
        }
        RegisterError::Io(error) => {
            tracing::error!(%parent, run, kind, %error, "could not record a run's job; the run goes on unlisted");
        }
    }
}

/// A run whose session's events the daemon streams.
struct Followed {
    /// What the run's child tells; `None` once the child let go of it.
    events: Option<ChildEvents>,
    stream: ChildStream,
}

impl Followed {
    /// The next thing the run's child tells; never ready once the child
    /// let go.
    async fn next(&mut self) -> ChildEvent {
        loop {
            match &mut self.events {
                Some(events) => match events.recv().await {
                    Some(event) => return event,
                    None => self.events = None,
                },
                None => return std::future::pending().await,
            }
        }
    }

    /// Shows the run's `status` where the stream carries it: its title,
    /// once the stream is open.
    fn show_status(&self, status: &RunStatus) {
        if let Some(title) = &status.title {
            self.stream.titled(title);
        }
    }

    /// Shows what the run's child told, with the run's `status` as it
    /// stands: a stream that opens takes the title the run already has.
    fn show(&self, event: ChildEvent, status: &RunStatus) {
        match event {
            ChildEvent::Opened(options) => {
                self.stream.open(*options);
                self.show_status(status);
            }
            ChildEvent::Session(event) => self.stream.publish([event]),
        }
    }

    /// Shows what the child told and this task has not read yet. The child
    /// tells everything before its run's status is terminal.
    fn show_rest(&mut self, status: &RunStatus) {
        while let Some(event) = self
            .events
            .as_mut()
            .and_then(|events| events.try_recv().ok())
        {
            self.show(event, status);
        }
    }

    /// Tells the run's clients its job moved to `state`, as `run` stands:
    /// a job that ended ends the run's turn, after the rest of its events.
    fn moved(&mut self, state: &JobState, run: &RunStatus) {
        match state {
            JobState::Ended(end) => {
                self.show_rest(run);
                self.stream.end(outcome(end, run), state);
            }
            JobState::Idle | JobState::Queued | JobState::Running | JobState::WaitingForInput => {
                self.stream.moved(state);
            }
        }
    }
}

/// How the turn of a run that ended as `end`, with status `run`, ended, as
/// its clients are told; `None` for one the daemon's stop interrupted,
/// whose job state says so.
fn outcome(end: &TurnEnd, run: &RunStatus) -> Option<TurnOutcome> {
    match end {
        // A run's status keeps its answer, not why the model stopped.
        TurnEnd::Completed => Some(TurnOutcome::Answered {
            text: run.result.clone().unwrap_or_default(),
            stop: AnswerStop::EndTurn,
        }),
        TurnEnd::Cancelled => Some(TurnOutcome::Cancelled),
        TurnEnd::Failed(message) => Some(TurnOutcome::Failed {
            error: TurnError {
                kind: TurnErrorKind::Other,
                message: message.clone(),
            },
        }),
        TurnEnd::Interrupted => None,
        // A run has no time limit of the daemon's.
        TurnEnd::TimedOut { .. } => None,
    }
}

/// Moves job `job` along with its run's `status` until the run ends,
/// streaming the events of a `followed` run to its clients meanwhile.
async fn follow(
    job: JobId,
    mut status: watch::Receiver<RunStatus>,
    mut followed: Option<Followed>,
    jobs: Arc<JobRegistry>,
    queues: Queues,
    stopping: CancellationToken,
) {
    let mut state = JobState::Queued;
    loop {
        let run = status.borrow_and_update().clone();
        if let Some(followed) = &followed {
            followed.show_status(&run);
        }
        if let Some(event) = next_event(&state, &run, stopping.is_cancelled()) {
            match jobs.record(&job, event) {
                Ok(next) => {
                    state = next.state;
                    if let Some(followed) = &mut followed {
                        followed.moved(&state, &run);
                    }
                }
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
        let changed = loop {
            tokio::select! {
                changed = status.changed() => break changed,
                event = next_child_event(&mut followed) => {
                    if let Some(followed) = &followed {
                        followed.show(event, &status.borrow());
                    }
                }
            }
        };
        if changed.is_err() {
            // The executor sends a terminal status before it lets go of
            // the run, even when the run panicked, so only a runtime that
            // shuts down under it gets here.
            let end = if stopping.is_cancelled() {
                TurnEnd::Interrupted
            } else {
                TurnEnd::Failed("the subagent run ended without a status".into())
            };
            match jobs.record(&job, JobEvent::TurnEnded(end)) {
                Ok(next) => {
                    if let Some(followed) = &mut followed {
                        followed.moved(&next.state, &run);
                    }
                }
                Err(error) => tracing::debug!(%job, %error, "subagent job already ended"),
            }
            break;
        }
    }
    if let Some(followed) = followed.filter(|_| stopping.is_cancelled()) {
        followed.stream.close(DetachReason::DaemonStopping);
    }
    lock(&queues).remove(&job);
}

/// The next thing a `followed` run's child tells; never ready for a run
/// nobody follows.
async fn next_child_event(followed: &mut Option<Followed>) -> ChildEvent {
    match followed {
        Some(followed) => followed.next().await,
        None => std::future::pending().await,
    }
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
