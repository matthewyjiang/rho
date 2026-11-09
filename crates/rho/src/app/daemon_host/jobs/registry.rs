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
    state::TransitionRefused, unix_now, JobEvent, JobId, JobRecord, JobState, JobStore, JobSubject,
    TurnEnd,
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

/// What the registry knows of a session, for reopening it (#1661).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) enum SessionJob {
    /// No job has run its turns, as for a session the TUI saved: a reopen
    /// registers one.
    New,
    /// The latest job of its turns, which a reopen records its next turns
    /// on.
    Last(JobRecord),
    /// This job writes the session right now, as a subagent run does, so
    /// the daemon must not open it a second time.
    Busy(JobId),
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
        };
        registry.interrupt_unfinished(&mut registry.lock());
        registry
    }

    /// Adds `record` and writes its file. Fails when the file cannot be
    /// written, and then the job is not added: a job the daemon could not
    /// record would vanish at its next restart.
    pub(in crate::app::daemon_host) fn register(
        &self,
        record: JobRecord,
    ) -> Result<(), RegisterError> {
        let mut jobs = self.lock();
        if self.closed.load(Ordering::Relaxed) {
            return Err(RegisterError::Closed);
        }
        self.store.write(&record)?;
        if record.state.in_progress() {
            self.started.fetch_add(1, Ordering::Relaxed);
        }
        jobs.insert(record.id.clone(), record);
        Ok(())
    }

    #[cfg(test)]
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
            // Whether the job runs the session's turns, of the jobs that
            // write it at all.
            let turns = match &record.subject {
                JobSubject::Session { session: written } if written == session => true,
                JobSubject::Subagent {
                    session: Some(written),
                } if written == session => false,
                JobSubject::Session { .. }
                | JobSubject::Subagent { .. }
                | JobSubject::Workflow { .. } => continue,
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
        last.map_or(SessionJob::New, |record| SessionJob::Last(record.clone()))
    }

    /// How many jobs have a turn under way.
    pub(in crate::app::daemon_host) fn in_progress(&self) -> usize {
        self.lock()
            .values()
            .filter(|record| record.state.in_progress())
            .count()
    }

    /// Starts a new turn on job `id`, unless one is already under way.
    /// Checked and applied under one lock, so of two prompts that race only
    /// one starts a turn.
    pub(in crate::app::daemon_host) fn start_turn(&self, id: &JobId) -> Result<(), StartTurnError> {
        let mut jobs = self.lock();
        if self.closed.load(Ordering::Relaxed) {
            return Err(StartTurnError::Closed);
        }
        match self.record_under(&mut jobs, id, JobEvent::TurnStarted) {
            Ok(_) => Ok(()),
            Err(TransitionError::Unknown(unknown)) => Err(unknown.into()),
            // A turn starts from every state but the two of a turn under way.
            Err(TransitionError::Refused(_)) => Err(StartTurnError::Busy),
        }
    }

    /// Applies `event` to job `id`, rewrites its record, and returns the
    /// state it left the job in.
    pub(in crate::app::daemon_host) fn record(
        &self,
        id: &JobId,
        event: JobEvent,
    ) -> Result<JobState, TransitionError> {
        self.record_under(&mut self.lock(), id, event)
    }

    /// [`Self::record`], with the `jobs` lock held.
    fn record_under(
        &self,
        jobs: &mut BTreeMap<JobId, JobRecord>,
        id: &JobId,
        event: JobEvent,
    ) -> Result<JobState, TransitionError> {
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        let was_in_progress = record.state.in_progress();
        apply(record, event)?;
        if record.state.in_progress() && !was_in_progress {
            self.started.fetch_add(1, Ordering::Relaxed);
        }
        persist(&self.store, record);
        Ok(record.state.clone())
    }

    /// Records that job `id`'s turn ended as `end`, which its clients were
    /// told as `outcome`, if they were, and returns the state it left the
    /// job in.
    pub(in crate::app::daemon_host) fn end_turn(
        &self,
        id: &JobId,
        end: TurnEnd,
        outcome: Option<rho_protocol::TurnOutcome>,
    ) -> Result<JobState, TransitionError> {
        let mut jobs = self.lock();
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        apply(record, JobEvent::TurnEnded(end))?;
        record.outcome = outcome;
        persist(&self.store, record);
        Ok(record.state.clone())
    }

    /// Forgets job `id` and deletes its record. A record that cannot be
    /// deleted is logged, and a restarted daemon lists the job again.
    pub(in crate::app::daemon_host) fn remove(&self, id: &JobId) {
        // Deleted under the lock every write takes, so no write of the job
        // races the delete and writes its record back.
        let mut jobs = self.lock();
        if jobs.remove(id).is_none() {
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

    /// Records every job in `jobs` with a turn under way as interrupted.
    fn interrupt_unfinished(&self, jobs: &mut BTreeMap<JobId, JobRecord>) {
        for record in jobs.values_mut() {
            // Refused exactly for the jobs with no turn under way.
            if apply(record, JobEvent::TurnEnded(TurnEnd::Interrupted)).is_ok() {
                persist(&self.store, record);
            }
        }
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
    let ends_turn = matches!(event, JobEvent::TurnEnded(_));
    record.state = record.state.after(event)?;
    if ends_turn {
        record.outcome = None;
    }
    record.updated_at = unix_now();
    Ok(())
}

/// Writes `record`'s file. A transition that cannot be written still
/// applies in memory: the job goes on, and its next transition rewrites the
/// file; until then a restarted daemon sees the job one step behind.
fn persist(store: &JobStore, record: &JobRecord) {
    if let Err(error) = store.write(record) {
        tracing::error!(job = %record.id, state = %record.state, %error, "could not write job record");
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
