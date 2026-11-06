use std::path::PathBuf;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{JobRegistry, StartTurnError};
use crate::app::daemon_host::jobs::{JobId, JobRecord, JobState, JobStore};

fn states(records: Vec<JobRecord>) -> Vec<(JobId, JobState)> {
    let mut states: Vec<_> = records
        .into_iter()
        .map(|record| (record.id, record.state))
        .collect();
    states.sort_by(|a, b| a.0.cmp(&b.0));
    states
}

// Covers: a daemon that starts over its predecessor's records lists every
// one, records each job that was running or waiting for input as
// interrupted, on disk as well as in memory, and leaves every other state
// as it was.
// Owner: daemon job registry startup.
#[test]
fn startup_interrupts_the_turns_a_dead_daemon_left_open() {
    use JobState::{Cancelled, Completed, Failed, Idle, Interrupted, Running, WaitingForInput};
    let home = TempDir::new().unwrap();
    let store = JobStore::in_rho_home(home.path());
    let cases = [
        (Idle, Idle),
        (Running, Interrupted),
        (WaitingForInput, Interrupted),
        (Completed, Completed),
        (Failed, Failed),
        (Cancelled, Cancelled),
        (Interrupted, Interrupted),
    ];
    let mut expected = Vec::new();
    for (saved, after) in cases {
        let record = JobRecord {
            state: saved,
            ..JobRecord::new_thread(PathBuf::from("/work"), rho_sdk::SessionId::new())
        };
        expected.push((record.id.clone(), after));
        store.write(&record).unwrap();
    }
    expected.sort_by(|a, b| a.0.cmp(&b.0));

    let registry = JobRegistry::open(store.clone());

    assert_eq!(states(registry.list()), expected);
    assert_eq!(states(store.load().unwrap()), expected);
    assert_eq!(registry.in_progress(), 0);
}

// Covers: a turn starts only on a job with none under way, so a second
// prompt to a running or waiting thread is refused as busy and leaves it as
// it was, and an unknown job is reported as unknown.
// Owner: daemon job registry.
#[test]
fn a_turn_starts_only_when_none_is_in_progress() {
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::in_rho_home(home.path()));
    let job = JobRecord::new_thread(PathBuf::from("/work"), rho_sdk::SessionId::new());
    let id = job.id.clone();
    registry.register(job).unwrap();

    let started = registry.start_turn(&id).map(|record| record.state);
    let while_running = registry.start_turn(&id).map(|record| record.state);
    registry.set_state(&id, JobState::WaitingForInput).unwrap();
    let while_waiting = registry.start_turn(&id).map(|record| record.state);
    let unknown = registry
        .start_turn(&JobId::generate())
        .map(|record| record.state);

    assert_eq!(
        [started, while_running, while_waiting, unknown],
        [
            Ok(JobState::Running),
            Err(StartTurnError::Busy),
            Err(StartTurnError::Busy),
            Err(StartTurnError::Unknown(super::UnknownJob)),
        ]
    );
    assert_eq!(
        registry.get(&id).map(|record| record.state),
        Some(JobState::WaitingForInput)
    );
}
