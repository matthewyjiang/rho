//! The subagent rail of a daemon session: the runs the session started,
//! as the daemon's jobs name them. The same job list feeds the workflow
//! rail (`daemon_workflows`).
//!
//! The daemon lists a job per run (`rho/job/list`), which says whose run it
//! is; what a run is doing is in its artifact directory, which the daemon
//! writes under the rho home this process shares. The rail asks for the
//! jobs when a session is first followed, once whenever a turn ends, and
//! while a turn runs or the last list named a run the rails show, the only
//! times one can start or change, at most once per [`RAIL_POLL`], and
//! reads the status of each run that is live or ended moments ago.
//! `/attach` then opens a run's artifacts from the same directory.
//!
//! In process, a finished row leaves the rail when its result reaches the
//! parent's transcript. The daemon delivers results itself, so here a
//! finished row leaves [`ENDED_RETENTION`] after its run ended.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use rho_protocol::{JobId, JobKind, JobState, JobSummary};

use super::App;
use crate::{
    session::SessionPersistence,
    subagent::{self, RunStatus},
    tools::agent::SubagentSnapshot,
};

/// The least time between two job lists: the tick the in-process rail
/// refreshes on while a subagent runs (`app_loop.rs`).
const RAIL_POLL: Duration = Duration::from_millis(500);

/// How long a finished run stays on the rail, as the in-process rail keeps
/// a run whose result it will not see delivered (`subagent_manager.rs`).
const ENDED_RETENTION: Duration = Duration::from_secs(10);

/// What the rail remembers between job lists.
#[derive(Default)]
pub(super) struct DaemonRail {
    /// When the last job list was asked for; `None` while one is owed
    /// whatever the session is doing: before the first, after a session
    /// switch, and once a turn ended.
    asked: Option<Instant>,
    /// Whether that list has not been answered yet.
    waiting: bool,
    /// Whether the last list named a run the rails show, which may change
    /// with no turn running. The list, not the rows, says so: a run whose
    /// files could not be read just then has no row and is still going.
    shows_runs: bool,
    /// Each run's artifact directory, resolved once.
    directories: HashMap<String, PathBuf>,
}

/// The jobs of `kind` the session `session` started itself, as the
/// in-process rails show them: those in progress, and those ended no longer
/// than [`ENDED_RETENTION`] before `now` (Unix seconds). A workflow's agent
/// nodes are not the session's own, and keep their artifacts elsewhere.
pub(super) fn rail_jobs<'a>(
    jobs: &'a [JobSummary],
    session: &str,
    kind: JobKind,
    now: u64,
) -> Vec<&'a JobSummary> {
    let own: HashSet<&JobId> = jobs
        .iter()
        .filter(|job| {
            job.kind == JobKind::Session
                && job
                    .session
                    .as_ref()
                    .is_some_and(|id| id.as_str() == session)
        })
        .map(|job| &job.id)
        .collect();
    jobs.iter()
        .filter(|job| {
            job.kind == kind
                && job
                    .parent
                    .as_ref()
                    .is_some_and(|parent| own.contains(parent))
        })
        .filter(|job| {
            in_progress(job.state) || now.saturating_sub(job.updated_at) < ENDED_RETENTION.as_secs()
        })
        .collect()
}

/// Whether a job in `state` may still change.
pub(super) fn in_progress(state: JobState) -> bool {
    match state {
        JobState::Idle | JobState::Queued | JobState::Running | JobState::WaitingForInput => true,
        JobState::Completed
        | JobState::Failed
        | JobState::Cancelled
        | JobState::TimedOut
        | JobState::Interrupted
        | JobState::Unknown => false,
        // A state this build does not know cannot be waited on.
        _ => false,
    }
}

impl DaemonRail {
    /// The status of `run`, read from its artifact directory.
    fn status(&mut self, run: &str) -> Option<RunStatus> {
        if !self.directories.contains_key(run) {
            let directory = subagent::resolve_run_directory(run).ok()?;
            self.directories.insert(run.to_owned(), directory);
        }
        subagent::read_status(&self.directories[run].join(subagent::RESULT_FILE_NAME))
    }
}

impl App {
    /// Asks the daemon for its jobs when the rail is due a refresh. The
    /// answer arrives as a view; see [`Self::show_daemon_jobs`].
    pub(super) fn refresh_daemon_rail(&mut self) {
        let Some(session) = &self.info.session.daemon else {
            return;
        };
        // A run starts only in a turn, and changes only while it shows.
        let may_change = self.turn.is_provider_turn() || self.daemon_rail.shows_runs;
        let rail = &mut self.daemon_rail;
        let due = rail
            .asked
            .is_none_or(|asked| may_change && asked.elapsed() >= RAIL_POLL);
        if rail.waiting || !due || session.request_jobs().is_err() {
            return;
        }
        rail.asked = Some(Instant::now());
        rail.waiting = true;
    }

    /// Owes the rail one more job list: a turn ended, and may have started
    /// a run since the last list, however briefly it ran.
    pub(super) fn daemon_turn_may_have_started_runs(&mut self) {
        self.daemon_rail.asked = None;
    }

    /// Whether the rails show runs that change with no turn running, so
    /// the idle loop keeps asking after them.
    pub(super) fn daemon_rail_shows_runs(&self) -> bool {
        self.daemon_rail.shows_runs
    }

    /// Puts the followed session's runs among `jobs` on the subagent and
    /// workflow rails. A failed list keeps the rows the rails have; the
    /// next one replaces them.
    pub(super) fn show_daemon_jobs(&mut self, jobs: Result<Vec<JobSummary>, String>) {
        self.daemon_rail.waiting = false;
        let (Ok(jobs), Some(session)) = (jobs, &self.info.session.daemon) else {
            return;
        };
        let session = session.id().to_owned();
        let now = subagent::unix_now_secs();
        let workflows = rail_jobs(&jobs, &session, JobKind::Workflow, now);
        let subagents = rail_jobs(&jobs, &session, JobKind::Subagent, now);
        self.daemon_rail.shows_runs = !workflows.is_empty() || !subagents.is_empty();
        self.show_daemon_workflows(&workflows, now);
        let snapshots = subagents
            .into_iter()
            .filter_map(|job| job.run.clone())
            .filter_map(|run| {
                let status = self.daemon_rail.status(&run)?;
                let done = status.state.is_terminal();
                let ended_long_ago = status.finished_at.is_some_and(|finished| {
                    now.saturating_sub(finished) >= ENDED_RETENTION.as_secs()
                });
                (!done || !ended_long_ago).then(|| SubagentSnapshot {
                    id: run,
                    agent_id: status.agent_id.clone().unwrap_or_else(|| "agent".into()),
                    title: status.title.clone(),
                    elapsed: status.elapsed_duration(now).unwrap_or_default(),
                    done,
                    prior_notices: Vec::new(),
                    // The rail does not show it.
                    persistence: SessionPersistence::Saved,
                    status,
                })
            })
            .collect();
        if self.subagent_panel.ingest(snapshots) {
            self.refresh_attach_picker();
        }
    }

    /// Forgets the session the rail showed, for the one followed next.
    pub(super) fn reset_daemon_rail(&mut self) {
        self.daemon_rail = DaemonRail::default();
        self.subagent_panel.ingest(Vec::new());
        self.workflow_panel.ingest(Vec::new());
    }
}

#[cfg(test)]
#[path = "daemon_subagents_tests.rs"]
mod tests;
