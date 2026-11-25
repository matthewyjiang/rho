//! Every job the daemon knows, in memory and in the store.

use std::{
    collections::BTreeMap,
    io,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, MutexGuard, PoisonError,
    },
};

use super::{
    state::TransitionRefused, unix_now, Admission, JobEvent, JobId, JobRecord, JobState, JobStore,
    JobSubject, Keeping, TurnEnd, TurnRecord,
};
use crate::{
    caller_context::{Access, CallerContext},
    daemon::Ending,
};

/// The daemon's jobs: the ones it runs and the records it loaded at
/// startup. Shared by every request and turn task.
///
/// Every change goes through one lock and rewrites the job's record before
/// the lock is released, so the file always holds the job's latest state
/// even when two changes race.
//
// Holding a std lock across the fsync blocks one runtime worker and any
// `job/list` or `status` caller for the write. Measured on ext4 (NVMe): a
// record-sized write, fsync, rename and directory fsync took 0.64 ms median
// and 1.9 ms max over 300 runs, so this stays until a profile says otherwise.
pub(in crate::app::daemon_host) struct JobRegistry {
    store: JobStore,
    jobs: Mutex<BTreeMap<JobId, JobRecord>>,
    /// Set, under the `jobs` lock, once the daemon's stop recorded its
    /// unfinished jobs, or an upgrade found none in progress; no job is
    /// added and no turn starts after.
    closed: AtomicBool,
    /// Counts the jobs that went in progress, under the `jobs` lock, so an
    /// upgrade can tell that none did since it last found the daemon idle.
    started: AtomicU64,
    /// Set, under the `jobs` lock, once the daemon's stop is a restart in
    /// place, whose interrupted session turns are marked to continue (see
    /// [`Self::stopping_for`]). Never set when the registry opens, so the
    /// turns a dead daemon left open are never marked.
    restarting: AtomicBool,
    /// Woken after every job is added, moves, or is forgotten.
    changed: tokio::sync::Notify,
}

/// The daemon found idle, as of [`JobRegistry::idle`]: no job was in
/// progress then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) struct Idle {
    started: u64,
}

/// Why a job was not added.
#[derive(Debug, thiserror::Error)]
pub(in crate::app::daemon_host) enum RegisterError {
    /// The daemon is stopping and takes no new jobs.
    #[error("the daemon is stopping")]
    Closed,
    /// The job's file could not be written.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// No job has this id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("no such job")]
pub(in crate::app::daemon_host) struct UnknownJob;

/// Why a turn could not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(in crate::app::daemon_host) enum StartTurnError {
    #[error(transparent)]
    Unknown(#[from] UnknownJob),
    /// A turn is already running or waiting for input.
    #[error("a turn is already in progress")]
    Busy,
    /// The daemon is stopping or restarting, and starts no turn.
    #[error("the daemon is stopping")]
    Closed,
}

/// Why a session may not answer another session's question
/// ([`JobRegistry::may_answer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) enum AnswerRefused {
    /// The answering session did not start the asking one: the asking job
    /// has no parent, its parent is gone, or it writes another session.
    NotStarter,
    /// The answering session started the asking one, but the grant it runs
    /// under may only watch sessions.
    ReadOnly,
    /// The answering session started the asking one, but the daemon knows
    /// no grant it runs under: it is a subagent run whose session's job is
    /// gone.
    NoGrant,
}

/// What the registry knows of a session, for reopening it (#1661).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) enum SessionJob {
    /// No job has run its turns, as for a session the TUI saved: a reopen
    /// registers one.
    New,
    /// The latest job of its turns, which a reopen records its next turns
    /// on.
    Last(Box<JobRecord>),
    /// This job writes the session right now, as a subagent run does, so
    /// the daemon must not open it a second time.
    Busy(JobId),
}

/// The session job a job runs under, as [`JobRegistry::runs_under`] finds
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) struct RunsUnder {
    /// The session whose turns that job runs.
    pub(in crate::app::daemon_host) session: rho_sdk::SessionId,
    /// The grant of that job's turn under way, or of its last one.
    pub(in crate::app::daemon_host) grant: CallerContext,
}

/// Where a change left a job: its state, and the turn the state is of,
/// which a session job's attached clients are told together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) struct Moved {
    pub(in crate::app::daemon_host) state: JobState,
    pub(in crate::app::daemon_host) turn: Option<rho_protocol::TurnId>,
}

impl JobRecord {
    /// Where the job is now. An idle job is of no turn, even one that
    /// keeps its last turn's record after its conversation moved.
    pub(in crate::app::daemon_host) fn moved(&self) -> Moved {
        let turn = match self.state {
            JobState::Idle => None,
            JobState::Queued
            | JobState::Running
            | JobState::WaitingForInput
            | JobState::Ended(_) => self.turn.as_ref().map(|turn| turn.id.clone()),
        };
        Moved {
            state: self.state.clone(),
            turn,
        }
    }
}

/// Why an event did not apply.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(in crate::app::daemon_host) enum TransitionError {
    #[error(transparent)]
    Unknown(#[from] UnknownJob),
    #[error(transparent)]
    Refused(#[from] TransitionRefused),
}

impl JobRegistry {
    /// The registry of every record in `store`. A job its last daemon left
    /// running or waiting for input is recorded `interrupted`, since no
    /// turn outlives the daemon that ran it. A store that cannot be read at
    /// all is logged and starts empty, as a malformed record is skipped.
    pub(in crate::app::daemon_host) fn open(store: JobStore) -> Self {
        let records = store.load().unwrap_or_else(|error| {
            tracing::error!(%error, "could not read job records; listing none from earlier daemons");
            Vec::new()
        });
        let registry = Self {
            store,
            jobs: Mutex::new(
                records
                    .into_iter()
                    .map(|record| (record.id.clone(), record))
                    .collect(),
            ),
            closed: AtomicBool::new(false),
            started: AtomicU64::new(0),
            restarting: AtomicBool::new(false),
            changed: tokio::sync::Notify::new(),
        };
        {
            let mut jobs = registry.lock();
            registry.interrupt_unfinished(&mut jobs);
            // A record written before turns had ids (#1777) names its last
            // turn from here on.
            for record in jobs.values_mut() {
                let ran_turns = matches!(record.subject, JobSubject::Session { .. })
                    && record.state != JobState::Idle;
                if ran_turns && record.turn.is_none() {
                    record.turn = Some(TurnRecord {
                        began: true,
                        ..TurnRecord::starting(/*key*/ None)
                    });
                    persist(&registry.store, record);
                }
            }
        }
        registry
    }

    /// Adds `record` and writes its file, unless it or the job that started
    /// it is kept in memory alone. Fails when the file cannot be written,
    /// and then the job is not added: a job the daemon could not record
    /// would vanish at its next restart.
    pub(in crate::app::daemon_host) fn register(
        &self,
        mut record: JobRecord,
    ) -> Result<(), RegisterError> {
        let mut jobs = self.lock();
        if self.closed.load(Ordering::Relaxed) {
            return Err(RegisterError::Closed);
        }
        let parent = record.parent.as_ref().and_then(|parent| jobs.get(parent));
        if parent.is_some_and(|parent| parent.keeping == Keeping::MemoryOnly) {
            record.keeping = Keeping::MemoryOnly;
        }
        match record.keeping {
            Keeping::Stored => self.store.write(&record)?,
            Keeping::MemoryOnly => {}
        }
        if record.state.in_progress() {
            self.started.fetch_add(1, Ordering::Relaxed);
        }
        jobs.insert(record.id.clone(), record);
        self.changed.notify_waiters();
        Ok(())
    }

    /// Resolves after the next change to any job: one added, moved, or
    /// forgotten. Enable it before reading the jobs it should follow, so a
    /// change in between is not missed.
    pub(in crate::app::daemon_host) fn changed(&self) -> tokio::sync::futures::Notified<'_> {
        self.changed.notified()
    }

    /// Whether job `id` runs a turn alone: `running`, with no job it
    /// started still queued, running, or waiting for input. Only then does
    /// a started session's turn hold its slot in the daemon-wide budget
    /// (#1714). A job it started may be one it does not wait on yet; the
    /// turn lends its slot all the same, since it cannot tell (see
    /// `sessions::budget`).
    pub(in crate::app::daemon_host) fn runs_alone(&self, id: &JobId) -> bool {
        let jobs = self.lock();
        jobs.get(id)
            .is_some_and(|record| record.state == JobState::Running)
            && !jobs
                .values()
                .any(|record| record.parent.as_ref() == Some(id) && record.state.in_progress())
    }

    /// Whether a job that writes `session` started a job still queued,
    /// running, or waiting for input, which keeps a merge from marking the
    /// session done (see `sessions::marks`). Every job that writes the
    /// session counts, its turns' and a subagent run's alike, so the answer
    /// does not turn on which of them is under way.
    pub(in crate::app::daemon_host) fn session_started_in_progress(
        &self,
        session: &rho_sdk::SessionId,
    ) -> bool {
        let jobs = self.lock();
        jobs.values().any(|record| {
            record.state.in_progress()
                && record
                    .parent
                    .as_ref()
                    .and_then(|parent| jobs.get(parent))
                    .is_some_and(|parent| parent.subject.session() == Some(session))
        })
    }

    /// Job `id` as the registry has it now.
    pub(in crate::app::daemon_host) fn get(&self, id: &JobId) -> Option<JobRecord> {
        self.lock().get(id).cloned()
    }

    /// Every job, oldest first.
    pub(in crate::app::daemon_host) fn list(&self) -> Vec<JobRecord> {
        let mut jobs: Vec<_> = self.lock().values().cloned().collect();
        jobs.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        jobs
    }

    /// What the registry knows of `session`: the job writing it, if one is
    /// under way, else the latest job of its turns.
    pub(in crate::app::daemon_host) fn session_job(
        &self,
        session: &rho_sdk::SessionId,
    ) -> SessionJob {
        let jobs = self.lock();
        let mut last: Option<&JobRecord> = None;
        for record in jobs.values() {
            if record.subject.session() != Some(session) {
                continue;
            }
            // Whether the job runs the session's turns, of the jobs that
            // write it at all.
            let turns = match record.subject {
                JobSubject::Session { .. } => true,
                JobSubject::Subagent { .. } | JobSubject::Workflow { .. } => false,
            };
            if record.state.in_progress() {
                return SessionJob::Busy(record.id.clone());
            }
            let later = last
                .is_none_or(|last| (last.created_at, &last.id) < (record.created_at, &record.id));
            if turns && later {
                last = Some(record);
            }
        }
        last.map_or(SessionJob::New, |record| {
            SessionJob::Last(Box::new(record.clone()))
        })
    }

    /// How many jobs have a turn under way.
    pub(in crate::app::daemon_host) fn in_progress(&self) -> usize {
        self.lock()
            .values()
            .filter(|record| record.state.in_progress())
            .count()
    }

    /// Records `grant`, held to the job's limit, as job `id`'s, as a
    /// session reopened under it has. Does nothing for a job the registry
    /// does not know.
    pub(in crate::app::daemon_host) fn set_grant(&self, id: &JobId, grant: CallerContext) {
        let mut jobs = self.lock();
        let Some(record) = jobs.get_mut(id) else {
            return;
        };
        let grant = record.held(grant);
        if record.grant != grant {
            record.grant = grant;
            persist(&self.store, record);
        }
    }

    /// Records that the daemon's cleanup removed job `id`'s worktree with
    /// `commit` checked out, or, with `None`, that it is on disk again.
    /// Not a transition: `updated_at` keeps when the job was last used.
    pub(in crate::app::daemon_host) fn set_worktree_removed_at(
        &self,
        id: &JobId,
        commit: Option<String>,
    ) {
        let mut jobs = self.lock();
        let Some(record) = jobs.get_mut(id) else {
            return;
        };
        if record.worktree_removed_at != commit {
            record.worktree_removed_at = commit;
            persist(&self.store, record);
        }
    }

    /// The session job whose turn job `id` runs under: itself, for a
    /// session job, else the nearest session job that started it, as a
    /// subagent run's or a workflow drive's is, and so a workflow node's,
    /// whose subagent job the drive started. `None` when no known session
    /// job is found.
    pub(in crate::app::daemon_host) fn runs_under(&self, id: &JobId) -> Option<RunsUnder> {
        let jobs = self.lock();
        let (session, grant) = runs_under(&jobs, id)?;
        Some(RunsUnder {
            session: session.clone(),
            grant: grant.clone(),
        })
    }

    /// Refuses session `by`'s answer to a question of the session whose
    /// turns job `asking` runs, unless the job that started `asking` writes
    /// `by`, as a session job or a subagent run does, and the grant that
    /// job runs under ([`Self::runs_under`]) may act on sessions. Only
    /// the grant's access counts, not the workspaces it reaches: starting
    /// the asking session already took reaching its workspace.
    pub(in crate::app::daemon_host) fn may_answer(
        &self,
        asking: &JobId,
        by: &rho_sdk::SessionId,
    ) -> Result<(), AnswerRefused> {
        let jobs = self.lock();
        let starter = jobs
            .get(asking)
            .and_then(|record| record.parent.as_ref())
            .and_then(|parent| jobs.get(parent))
            .filter(|parent| parent.subject.session() == Some(by))
            .ok_or(AnswerRefused::NotStarter)?;
        match runs_under(&jobs, &starter.id).map(|(_, grant)| grant.scope.access) {
            Some(Access::Write) => Ok(()),
            Some(Access::Read) => Err(AnswerRefused::ReadOnly),
            None => Err(AnswerRefused::NoGrant),
        }
    }

    /// The latest subagent run, a workflow node's included, that wrote
    /// `session` and has ended: the session it ran under
    /// ([`Self::runs_under`]) owns `session` once no job writes it any
    /// more. `None` when no such run is known.
    pub(in crate::app::daemon_host) fn last_run_writing(
        &self,
        session: &rho_sdk::SessionId,
    ) -> Option<JobId> {
        let jobs = self.lock();
        jobs.values()
            .filter(|record| {
                !record.state.in_progress()
                    && matches!(
                        &record.subject,
                        JobSubject::Subagent { session: Some(written), .. } if written == session
                    )
            })
            .max_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)))
            .map(|record| record.id.clone())
    }

    /// The grant job `id`'s record holds, if the job is known.
    pub(in crate::app::daemon_host) fn grant_of(&self, id: &JobId) -> Option<CallerContext> {
        self.lock().get(id).map(|record| record.grant.clone())
    }

    /// Starts a new turn, `turn`, on session job `id`, asking for `grant`,
    /// unless a turn is already under way. The job's record keeps the grant
    /// from then on, held to the job's limit (#1796). Returns the state the
    /// turn starts in, `queued` for a session another job started, whose
    /// turns wait for capacity in the daemon-wide budget (#1714), `running`
    /// otherwise, and the grant the turn got. Checked and
    /// applied under one lock, so of two prompts that race only one starts
    /// a turn.
    pub(in crate::app::daemon_host) fn start_turn(
        &self,
        id: &JobId,
        turn: TurnRecord,
        grant: CallerContext,
    ) -> Result<(Moved, CallerContext), StartTurnError> {
        let mut jobs = self.lock();
        if self.closed.load(Ordering::Relaxed) {
            return Err(StartTurnError::Closed);
        }
        let event = match jobs.get(id).map(|record| record.admission) {
            Some(Admission::SharedBudget) => JobEvent::TurnQueued,
            Some(Admission::Immediate) | None => JobEvent::TurnStarted,
        };
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        let was_in_progress = record.state.in_progress();
        // A turn starts from every state but those of a turn under way.
        self.transition(record, event)
            .map_err(|_| StartTurnError::Busy)?;
        // The outcome kept was the last turn's.
        record.outcome = None;
        record.turn = Some(turn);
        record.grant = record.held(grant);
        let grant = record.grant.clone();
        Ok((self.wrote(record, was_in_progress), grant))
    }

    /// Records that job `id`'s turn `turn` began, so a later daemon still
    /// answers a prompt sent again with the turn's key instead of running
    /// it. Does nothing once it is recorded, or once another turn started.
    pub(in crate::app::daemon_host) fn turn_began(&self, id: &JobId, turn: &rho_protocol::TurnId) {
        let mut jobs = self.lock();
        if let Some(record) = jobs.get_mut(id) {
            let current = record.turn.as_mut();
            if let Some(turn) = current.filter(|current| current.id == *turn && !current.began) {
                turn.began = true;
                persist(&self.store, record);
            }
        }
    }

    /// Applies `event` to job `id`, rewrites its record, and returns where
    /// it left the job.
    pub(in crate::app::daemon_host) fn record(
        &self,
        id: &JobId,
        event: JobEvent,
    ) -> Result<Moved, TransitionError> {
        let mut jobs = self.lock();
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        let was_in_progress = record.state.in_progress();
        self.transition(record, event)?;
        Ok(self.wrote(record, was_in_progress))
    }

    /// Writes `record`, just moved from a state that `was_in_progress` or
    /// not, under the `jobs` lock, and returns where it is now.
    fn wrote(&self, record: &JobRecord, was_in_progress: bool) -> Moved {
        if record.state.in_progress() && !was_in_progress {
            self.started.fetch_add(1, Ordering::Relaxed);
        }
        persist(&self.store, record);
        self.changed.notify_waiters();
        record.moved()
    }

    /// Records that job `id`'s turn ended as `end`, which its clients were
    /// told as `outcome`, if they were, and returns where it left the job.
    pub(in crate::app::daemon_host) fn end_turn(
        &self,
        id: &JobId,
        end: TurnEnd,
        outcome: Option<rho_protocol::TurnOutcome>,
    ) -> Result<Moved, TransitionError> {
        let mut jobs = self.lock();
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        self.transition(record, JobEvent::TurnEnded(end))?;
        record.outcome = outcome;
        persist(&self.store, record);
        self.changed.notify_waiters();
        Ok(record.moved())
    }

    /// Rests job `id` idle, without its last turn's outcome, once the
    /// session's conversation moved to a point of its tree that may not
    /// hold that turn, so a later daemon that reopens the session neither
    /// tells its clients an answer the conversation lacks nor that the turn
    /// ended. The job keeps the turn's record, which a prompt sent again
    /// with its key still finds.
    pub(in crate::app::daemon_host) fn conversation_moved(
        &self,
        id: &JobId,
    ) -> Result<(), TransitionError> {
        let mut jobs = self.lock();
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        self.transition(record, JobEvent::ConversationMoved)?;
        persist(&self.store, record);
        self.changed.notify_waiters();
        Ok(())
    }

    /// Forgets job `id` and deletes its record. A record that cannot be
    /// deleted is logged, and a restarted daemon lists the job again.
    pub(in crate::app::daemon_host) fn remove(&self, id: &JobId) {
        // Deleted under the lock every write takes, so no write of the job
        // races the delete and writes its record back.
        let mut jobs = self.lock();
        let Some(removed) = jobs.remove(id) else {
            return;
        };
        self.changed.notify_waiters();
        if removed.keeping == Keeping::MemoryOnly {
            return;
        }
        if let Err(error) = self.store.remove(id) {
            tracing::error!(job = %id, %error, "could not delete job record");
        }
    }

    /// For an upgrade: the daemon as idle now, if no job is in progress.
    pub(in crate::app::daemon_host) fn idle(&self) -> Option<Idle> {
        let jobs = self.lock();
        (!jobs.values().any(|record| record.state.in_progress())).then(|| Idle {
            started: self.started.load(Ordering::Relaxed),
        })
    }

    /// For an upgrade: closes the registry, as [`Self::close`] would, but
    /// only while it stayed `idle`, no job having gone in progress since;
    /// whether it did. Checked and applied under one lock, so a turn either
    /// started before, and keeps the upgrade waiting, or is refused after,
    /// and the upgrade's checks between `idle` and this saw no turn's
    /// leftovers appear.
    pub(in crate::app::daemon_host) fn close_if_still(&self, idle: Idle) -> bool {
        let _jobs = self.lock();
        let still = self.started.load(Ordering::Relaxed) == idle.started;
        if still {
            self.closed.store(true, Ordering::Relaxed);
        }
        still
    }

    /// For the daemon's stop, once its turns saved: records every job
    /// still under way as interrupted, and takes no new job after, so a
    /// run a workflow drive starts during the stop is not left listed as
    /// running (#1658).
    pub(in crate::app::daemon_host) fn close(&self) {
        let mut jobs = self.lock();
        self.closed.store(true, Ordering::Relaxed);
        self.interrupt_unfinished(&mut jobs);
    }

    /// Records how the daemon's stop, about to begin, ends. Once it is a
    /// restart in place, each session turn interrupted from then on, by the
    /// stop or as it closes the registry, is marked for the restarted daemon
    /// to continue (see `sessions/restart_resume.rs`) if its record says it
    /// [`TurnRecord::continues_after_restart`]; subagent and workflow
    /// jobs are not, since the turn that started them, or the workflow's
    /// own recovery, decides what becomes of them. A stop asked for after a
    /// restart marks no more; the marks a restart already made are cleared
    /// by the next start, which is not a restart's.
    pub(in crate::app::daemon_host) fn stopping_for(&self, ending: Ending) {
        let _jobs = self.lock();
        let restarting = match ending {
            Ending::Stop => false,
            Ending::Restart => true,
        };
        self.restarting.store(restarting, Ordering::Relaxed);
    }

    /// The sessions whose turn the last daemon's restart in place
    /// interrupted, as their jobs' marks say. Clears every mark, on disk
    /// too, before the caller continues any turn, so a daemon that dies
    /// continuing them never continues them again. Call once, as the daemon
    /// starts: every start clears the marks, a restart's or not.
    pub(in crate::app::daemon_host) fn take_restart_interrupted(&self) -> Vec<rho_sdk::SessionId> {
        let mut jobs = self.lock();
        let mut sessions = Vec::new();
        for record in jobs.values_mut() {
            if !record.resume_after_restart {
                continue;
            }
            record.resume_after_restart = false;
            persist(&self.store, record);
            match &record.subject {
                JobSubject::Session { session } => sessions.push(session.clone()),
                // Only a session job is ever marked.
                JobSubject::Subagent { .. } | JobSubject::Workflow { .. } => {}
            }
        }
        sessions
    }

    /// Records every job in `jobs` with a turn under way as interrupted.
    fn interrupt_unfinished(&self, jobs: &mut BTreeMap<JobId, JobRecord>) {
        for record in jobs.values_mut() {
            // Refused exactly for the jobs with no turn under way.
            if self
                .transition(record, JobEvent::TurnEnded(TurnEnd::Interrupted))
                .is_ok()
            {
                persist(&self.store, record);
            }
        }
    }

    /// Moves `record` on by `event`, as [`apply`] does, under the `jobs`
    /// lock, and sets its restart mark, `resume_after_restart`: set when the
    /// daemon's restart in place interrupts a session turn that
    /// [`TurnRecord::continues_after_restart`] (see [`Self::stopping_for`]),
    /// cleared by every other transition. Every transition goes through
    /// here, so no mark outlives the interruption that set it.
    fn transition(&self, record: &mut JobRecord, event: JobEvent) -> Result<(), TransitionRefused> {
        let marks = matches!(event, JobEvent::TurnEnded(TurnEnd::Interrupted))
            && matches!(record.subject, JobSubject::Session { .. })
            && record
                .turn
                .as_ref()
                .is_some_and(|turn| turn.continues_after_restart)
            && self.restarting.load(Ordering::Relaxed);
        apply(record, event)?;
        record.resume_after_restart = marks;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<JobId, JobRecord>> {
        // A panic mid-update leaves at worst a record one transition
        // behind, which the next transition rewrites.
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Moves `record` on by `event`. An ended turn forgets the last one's
/// outcome; [`JobRegistry::end_turn`] then records its own.
fn apply(record: &mut JobRecord, event: JobEvent) -> Result<(), TransitionRefused> {
    let drops_outcome = matches!(event, JobEvent::TurnEnded(_) | JobEvent::ConversationMoved);
    record.state = record.state.after(event)?;
    if drops_outcome {
        record.outcome = None;
    }
    record.updated_at = unix_now();
    Ok(())
}

/// Writes `record`'s file, if it has one. A transition that cannot be written still
/// applies in memory: the job goes on, and its next transition rewrites the
/// file; until then a restarted daemon sees the job one step behind.
fn persist(store: &JobStore, record: &JobRecord) {
    if record.keeping == Keeping::MemoryOnly {
        return;
    }
    if let Err(error) = store.write(record) {
        tracing::error!(job = %record.id, state = %record.state, %error, "could not write job record");
    }
}

/// The session and the grant of the session job that job `id` runs under,
/// in `jobs`: see [`JobRegistry::runs_under`].
fn runs_under<'a>(
    jobs: &'a BTreeMap<JobId, JobRecord>,
    id: &JobId,
) -> Option<(&'a rho_sdk::SessionId, &'a CallerContext)> {
    let mut record = jobs.get(id)?;
    loop {
        match &record.subject {
            JobSubject::Session { session } => return Some((session, &record.grant)),
            JobSubject::Subagent { .. } | JobSubject::Workflow { .. } => {
                record = jobs.get(record.parent.as_ref()?)?;
            }
        }
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
