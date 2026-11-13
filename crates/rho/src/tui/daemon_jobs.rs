//! The rails of the session: the subagent and workflow runs the
//! session started, as the daemon's jobs name them, and the background
//! processes it started (`daemon_processes`).
//!
//! The daemon lists a job per subagent run and per drive of a workflow run
//! (`rho/job/list`), which says whose it is; what a run is doing is in the
//! files the daemon writes under the rho home this process shares
//! (`daemon_subagents`, `daemon_workflows`). The rails ask for the jobs
//! when a session is first followed, once whenever a turn ends, and while a
//! turn runs or the last list named a run the rails show, the only times
//! one can start or change, at most once per [`RAIL_POLL`].
//!
//! The daemon delivers results itself. A finished subagent row leaves when
//! the transcript shows its result, or [`ENDED_RETENTION`] after its run
//! ended; a workflow row leaves once the daemon no longer lists its result
//! as pending (`daemon_workflows`).

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use rho_protocol::{JobId, JobKind, JobState, JobSummary};

use super::{daemon_subagents::SubagentRuns, daemon_workflows::WorkflowRuns, App};
use crate::subagent;

/// The least time between two job lists: the idle loop's tick while a rail
/// shows runs (`app_loop.rs`).
const RAIL_POLL: Duration = Duration::from_millis(500);

/// How long a finished run stays on its rail, as the subagent manager's
/// rail keeps a run whose result it will not see delivered
/// (`subagent_manager.rs`).
pub(super) const ENDED_RETENTION: Duration = Duration::from_secs(10);

/// When a rail asks the daemon for its rows: when a session is first
/// followed, once whenever a turn ends, and while its rows may change, at
/// most once per [`RAIL_POLL`].
#[derive(Default)]
pub(super) struct RailPoll {
    /// When the rows were last asked for; `None` while a read is owed
    /// whatever the session is doing: before the first, after a session
    /// switch, and once a turn ended.
    asked: Option<Instant>,
    /// Whether that read has not been answered yet.
    waiting: bool,
    /// Whether the last answer named a row the rail shows, which may change
    /// with no turn running, or failed, so that nothing is known. The
    /// answer, not the rows, says so: a run whose files could not be read
    /// just then has no row and is still going.
    shows: bool,
}

impl RailPoll {
    /// Whether to ask now. `may_change` says the rows may have changed
    /// since the last answer for a reason the answer could not know.
    pub(super) fn due(&self, may_change: bool) -> bool {
        !self.waiting
            && self
                .asked
                .is_none_or(|asked| (may_change || self.shows) && asked.elapsed() >= RAIL_POLL)
    }

    pub(super) fn asked(&mut self) {
        self.asked = Some(Instant::now());
        self.waiting = true;
    }

    /// Takes an answer; `shows` as the field says.
    pub(super) fn answered(&mut self, shows: bool) {
        self.waiting = false;
        self.shows = shows;
    }

    /// Owes the rail one more read, whatever the session is doing.
    pub(super) fn owe(&mut self) {
        self.asked = None;
    }
}

/// What the rails remember between reads.
#[derive(Default)]
pub(super) struct DaemonRail {
    jobs: RailPoll,
    pub(super) processes: RailPoll,
    pub(super) subagents: SubagentRuns,
    pub(super) workflows: WorkflowRuns,
}

impl DaemonRail {
    /// Owes the rails one more read each: a turn ended, and may have
    /// started a run or a process since the last, however briefly it ran.
    pub(super) fn turn_ended(&mut self) {
        self.jobs.owe();
        self.processes.owe();
    }

    /// Whether the rails show runs that change with no turn running, so
    /// the idle loop keeps asking after them. A process row keeps the loop
    /// ticking itself.
    pub(super) fn shows_runs(&self) -> bool {
        self.jobs.shows
    }
}

/// The jobs of `kind` the session `session` started itself that a rail
/// shows: those that may still change, those whose result the session has
/// yet to be given, and those ended no longer than [`ENDED_RETENTION`]
/// before `now` (Unix seconds). A workflow's agent
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
            may_still_change(job.state)
                || job.result_pending
                || now.saturating_sub(job.updated_at) < ENDED_RETENTION.as_secs()
        })
        .collect()
}

/// Whether a job in `state` may still change.
pub(super) fn may_still_change(state: JobState) -> bool {
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

impl App {
    /// Asks the daemon for its jobs and the session's processes when the
    /// rails are due a refresh. The answers arrive as views; see
    /// [`Self::show_daemon_jobs`] and [`Self::show_daemon_processes`].
    pub(super) fn refresh_daemon_rail(&mut self) {
        self.refresh_daemon_processes();
        let session = &self.info.session.daemon;
        // A run starts only in a turn, and changes only while it shows. An
        // open `/jobs` lists every session's jobs, which change whatever
        // this one is doing.
        let may_change = self.turn.is_provider_turn() || self.jobs_view_open();
        let jobs = &mut self.daemon_rail.jobs;
        if jobs.due(may_change) && session.request_jobs().is_ok() {
            jobs.asked();
        }
    }

    /// Owes the job list one more read, whatever the session is doing.
    pub(super) fn owe_daemon_jobs(&mut self) {
        self.daemon_rail.jobs.owe();
    }

    /// Puts the followed session's runs among `jobs` on the subagent and
    /// workflow rails. A failed list keeps the rows the rails have and is
    /// asked for again; the next one replaces them.
    pub(super) fn show_daemon_jobs(&mut self, jobs: Result<Vec<JobSummary>, String>) {
        let Ok(jobs) = jobs else {
            // Whether a run shows is unknown, such as one the turn that
            // just ended started.
            self.daemon_rail.jobs.answered(/*shows*/ true);
            return;
        };
        let now = subagent::unix_now_secs();
        // A session not opened yet runs nothing.
        let (workflows, subagents) = match self.info.session.daemon.id() {
            Some(session) => (
                rail_jobs(&jobs, session, JobKind::Workflow, now),
                rail_jobs(&jobs, session, JobKind::Subagent, now),
            ),
            None => (Vec::new(), Vec::new()),
        };
        self.daemon_rail
            .jobs
            .answered(!workflows.is_empty() || !subagents.is_empty());
        self.show_daemon_workflows(&workflows, now);
        self.show_daemon_subagents(&subagents, now);
    }

    /// Forgets the session the rails showed, for the one followed next.
    pub(super) fn reset_daemon_rail(&mut self) {
        self.daemon_rail = DaemonRail::default();
        self.subagent_panel.ingest(Vec::new());
        self.workflow_panel.ingest(Vec::new());
        self.process_panel.ingest(Vec::new());
    }
}

#[cfg(test)]
#[path = "daemon_jobs_tests.rs"]
mod tests;
