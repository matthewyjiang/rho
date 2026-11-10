//! The subagent rail of a daemon session: the runs the session started,
//! as the daemon's jobs name them.
//!
//! The daemon lists a job per run (`rho/job/list`), which says whose run it
//! is; what a run is doing is in its artifact directory, which the daemon
//! writes under the rho home this process shares. The rail asks for the
//! jobs while the activity panels refresh, at most once per [`RAIL_POLL`],
//! and reads the status of each run that is live or ended moments ago.
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
    /// When the last job list was asked for; `None` before the first, and
    /// after a session switch.
    asked: Option<Instant>,
    /// Whether that list has not been answered yet.
    waiting: bool,
    /// Each run's artifact directory, resolved once.
    directories: HashMap<String, PathBuf>,
}

/// The runs of the subagent jobs under `session`'s jobs, at any depth, so
/// a workflow run's agent nodes count: those in progress, and those ended
/// no longer than [`ENDED_RETENTION`] before `now` (Unix seconds).
fn rail_runs(jobs: &[JobSummary], session: &str, now: u64) -> Vec<String> {
    let mut family: HashSet<&JobId> = jobs
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
    let child_of = |family: &HashSet<&JobId>, job: &JobSummary| {
        job.parent
            .as_ref()
            .is_some_and(|parent| family.contains(parent))
    };
    // Jobs made in the same second list in either order, so a pass may
    // meet a job before its parent: repeat until one adds nothing.
    loop {
        let children: Vec<&JobId> = jobs
            .iter()
            .filter(|job| child_of(&family, job) && !family.contains(&job.id))
            .map(|job| &job.id)
            .collect();
        if children.is_empty() {
            break;
        }
        family.extend(children);
    }
    jobs.iter()
        .filter(|job| job.kind == JobKind::Subagent && child_of(&family, job))
        .filter(|job| {
            let in_progress = matches!(
                job.state,
                JobState::Idle | JobState::Queued | JobState::Running | JobState::WaitingForInput
            );
            in_progress || now.saturating_sub(job.updated_at) < ENDED_RETENTION.as_secs()
        })
        .filter_map(|job| job.run.clone())
        .collect()
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
        let rail = &mut self.daemon_rail;
        let due = rail.asked.is_none_or(|asked| asked.elapsed() >= RAIL_POLL);
        if rail.waiting || !due || session.request_jobs().is_err() {
            return;
        }
        rail.asked = Some(Instant::now());
        rail.waiting = true;
    }

    /// Puts the followed session's runs among `jobs` on the subagent rail.
    /// A failed list keeps the rows the rail has; the next one replaces
    /// them.
    pub(super) fn show_daemon_jobs(&mut self, jobs: Result<Vec<JobSummary>, String>) {
        self.daemon_rail.waiting = false;
        let (Ok(jobs), Some(session)) = (jobs, &self.info.session.daemon) else {
            return;
        };
        let now = subagent::unix_now_secs();
        let snapshots = rail_runs(&jobs, session.id(), now)
            .into_iter()
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
    }
}

#[cfg(test)]
#[path = "daemon_subagents_tests.rs"]
mod tests;
