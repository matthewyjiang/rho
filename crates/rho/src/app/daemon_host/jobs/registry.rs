//! Every job the daemon knows, in memory and in the store.

use std::{
    collections::BTreeMap,
    io,
    sync::{Mutex, MutexGuard, PoisonError},
};

use super::{state::TransitionRefused, unix_now, JobId, JobRecord, JobState, JobStore};

/// The daemon's jobs: the ones it runs and the records it loaded at
/// startup. Shared by every request and turn task.
///
/// Every change goes through one lock and rewrites the job's record before
/// the lock is released, so the file always holds the job's latest state
/// even when two changes race. A record write is one small file, so holding
/// the lock across it costs little.
pub(in crate::app::daemon_host) struct JobRegistry {
    store: JobStore,
    jobs: Mutex<BTreeMap<JobId, JobRecord>>,
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
}

/// Why a transition did not apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
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
        let mut jobs = BTreeMap::new();
        for mut record in records {
            if record.state.in_progress() {
                apply(&mut record, JobState::Interrupted, None)
                    .expect("an in-progress job can always be interrupted");
                persist(&store, &record);
            }
            jobs.insert(record.id.clone(), record);
        }
        Self {
            store,
            jobs: Mutex::new(jobs),
        }
    }

    /// Adds `record` and writes its file. Fails when the file cannot be
    /// written, and then the job is not added: a job the daemon could not
    /// record would vanish at its next restart.
    pub(in crate::app::daemon_host) fn register(&self, record: JobRecord) -> io::Result<()> {
        let mut jobs = self.lock();
        self.store.write(&record)?;
        jobs.insert(record.id.clone(), record);
        Ok(())
    }

    pub(in crate::app::daemon_host) fn get(&self, id: &JobId) -> Option<JobRecord> {
        self.lock().get(id).cloned()
    }

    /// Every job, oldest first.
    pub(in crate::app::daemon_host) fn list(&self) -> Vec<JobRecord> {
        let mut jobs: Vec<_> = self.lock().values().cloned().collect();
        jobs.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        jobs
    }

    /// How many jobs have a turn under way.
    pub(in crate::app::daemon_host) fn in_progress(&self) -> usize {
        self.lock()
            .values()
            .filter(|record| record.state.in_progress())
            .count()
    }

    /// Moves job `id` to `running` for a new turn, unless one is already
    /// under way. Checked and applied under one lock, so of two prompts
    /// that race only one starts a turn.
    pub(in crate::app::daemon_host) fn start_turn(
        &self,
        id: &JobId,
    ) -> Result<JobRecord, StartTurnError> {
        let mut jobs = self.lock();
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        if record.state.in_progress() {
            return Err(StartTurnError::Busy);
        }
        apply(record, JobState::Running, None).expect("a turn starts from any ended state");
        persist(&self.store, record);
        Ok(record.clone())
    }

    /// Moves job `id` to `to`. Use [`Self::fail`] for `failed`, which
    /// records why.
    pub(in crate::app::daemon_host) fn set_state(
        &self,
        id: &JobId,
        to: JobState,
    ) -> Result<JobRecord, TransitionError> {
        self.update(id, to, None)
    }

    /// Moves job `id` to `failed` because of `error`.
    pub(in crate::app::daemon_host) fn fail(
        &self,
        id: &JobId,
        error: String,
    ) -> Result<JobRecord, TransitionError> {
        self.update(id, JobState::Failed, Some(error))
    }

    fn update(
        &self,
        id: &JobId,
        to: JobState,
        error: Option<String>,
    ) -> Result<JobRecord, TransitionError> {
        let mut jobs = self.lock();
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        apply(record, to, error)?;
        persist(&self.store, record);
        Ok(record.clone())
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<JobId, JobRecord>> {
        // A panic mid-update leaves at worst a record one transition
        // behind, which the next transition rewrites.
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Moves `record` to `to`, with `error` as why it failed.
fn apply(
    record: &mut JobRecord,
    to: JobState,
    error: Option<String>,
) -> Result<(), TransitionRefused> {
    record.state = record.state.transition(to)?;
    record.error = error;
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
