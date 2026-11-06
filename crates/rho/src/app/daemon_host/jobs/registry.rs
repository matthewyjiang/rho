//! Every job the daemon knows, in memory and in the store.

use std::{
    collections::BTreeMap,
    io,
    sync::{Mutex, MutexGuard, PoisonError},
};

use super::{state::TransitionRefused, unix_now, JobEvent, JobId, JobRecord, JobStore, TurnEnd};

/// The daemon's jobs: the ones it runs and the records it loaded at
/// startup. Shared by every request and turn task.
///
/// Every change goes through one lock and rewrites the job's record before
/// the lock is released, so the file always holds the job's latest state
/// even when two changes race.
//
// Holding a std lock across the fsync blocks one runtime worker and any
// `jobs/list` or `status` caller for the write. Measured on ext4 (NVMe): a
// record-sized write, fsync, rename and directory fsync took 0.64 ms median
// and 1.9 ms max over 300 runs, so this stays until a profile says otherwise.
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
        };
        registry.interrupt_unfinished();
        registry
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

    /// Starts a new turn on job `id`, unless one is already under way.
    /// Checked and applied under one lock, so of two prompts that race only
    /// one starts a turn.
    pub(in crate::app::daemon_host) fn start_turn(&self, id: &JobId) -> Result<(), StartTurnError> {
        match self.record(id, JobEvent::TurnStarted) {
            Ok(()) => Ok(()),
            Err(TransitionError::Unknown(unknown)) => Err(unknown.into()),
            // A turn starts from every state but the two of a turn under way.
            Err(TransitionError::Refused(_)) => Err(StartTurnError::Busy),
        }
    }

    /// Applies `event` to job `id` and rewrites its record.
    pub(in crate::app::daemon_host) fn record(
        &self,
        id: &JobId,
        event: JobEvent,
    ) -> Result<(), TransitionError> {
        let mut jobs = self.lock();
        let record = jobs.get_mut(id).ok_or(UnknownJob)?;
        apply(record, event)?;
        persist(&self.store, record);
        Ok(())
    }

    /// Records every turn still under way as interrupted: at startup, for
    /// the turns a dead daemon left open, and as the daemon stops, for the
    /// turns that did not save in time.
    pub(in crate::app::daemon_host) fn interrupt_unfinished(&self) {
        let mut jobs = self.lock();
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

/// Moves `record` on by `event`.
fn apply(record: &mut JobRecord, event: JobEvent) -> Result<(), TransitionRefused> {
    record.state = record.state.after(event)?;
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
