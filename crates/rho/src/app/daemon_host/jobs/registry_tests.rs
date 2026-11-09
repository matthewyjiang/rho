use std::path::PathBuf;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{JobRegistry, RegisterError, SessionJob, StartTurnError};
use crate::app::daemon_host::jobs::{JobEvent, JobId, JobRecord, JobState, JobStore, TurnEnd};

fn states(records: Vec<JobRecord>) -> Vec<(JobId, JobState)> {
    let mut states: Vec<_> = records
        .into_iter()
        .map(|record| (record.id, record.state))
        .collect();
    states.sort_by(|a, b| a.0.cmp(&b.0));
    states
}

// Covers: a daemon that starts over its predecessor's records lists every
// one, records each job that was queued, running, or waiting for input as
// interrupted, on disk as well as in memory, and leaves every other state
// as it was.
// Owner: daemon job registry startup.
#[test]
fn startup_interrupts_the_turns_a_dead_daemon_left_open() {
    use JobState::{Ended, Idle, Queued, Running, WaitingForInput};
    let home = TempDir::new().unwrap();
    let store = JobStore::open(home.path()).unwrap();
    let failed = Ended(TurnEnd::Failed("provider refused the request".into()));
    let cases = [
        (Idle, Idle),
        (Queued, Ended(TurnEnd::Interrupted)),
        (Running, Ended(TurnEnd::Interrupted)),
        (WaitingForInput, Ended(TurnEnd::Interrupted)),
        (Ended(TurnEnd::Completed), Ended(TurnEnd::Completed)),
        (failed.clone(), failed),
        (Ended(TurnEnd::Cancelled), Ended(TurnEnd::Cancelled)),
        (Ended(TurnEnd::Interrupted), Ended(TurnEnd::Interrupted)),
    ];
    let mut expected = Vec::new();
    for (saved, after) in cases {
        let record = JobRecord {
            state: saved,
            ..JobRecord::new_session(
                JobId::generate(),
                PathBuf::from("/work"),
                rho_sdk::SessionId::new(),
            )
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
// prompt to a running or waiting session is refused as busy and leaves it as
// it was, and an unknown job is reported as unknown.
// Owner: daemon job registry.
#[test]
fn a_turn_starts_only_when_none_is_in_progress() {
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let job = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let id = job.id.clone();
    registry.register(job).unwrap();

    let started = registry.start_turn(&id);
    let while_running = registry.start_turn(&id);
    registry.record(&id, JobEvent::InputOpened).unwrap();
    let while_waiting = registry.start_turn(&id);
    let unknown = registry.start_turn(&JobId::generate());

    assert_eq!(
        [started, while_running, while_waiting, unknown],
        [
            Ok(()),
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

// Covers: the daemon's stop records a running job as interrupted, on disk
// too, and refuses every job registered after it, so a run started during
// the stop is never left listed as running.
// Owner: daemon job registry stop.
#[test]
fn a_closed_registry_interrupts_its_turns_and_takes_no_new_jobs() {
    let home = TempDir::new().unwrap();
    let store = JobStore::open(home.path()).unwrap();
    let registry = JobRegistry::open(store.clone());
    let job = |state| JobRecord {
        state,
        ..JobRecord::new_session(
            JobId::generate(),
            PathBuf::from("/work"),
            rho_sdk::SessionId::new(),
        )
    };
    let running = job(JobState::Running);
    let expected = vec![(running.id.clone(), JobState::Ended(TurnEnd::Interrupted))];
    registry.register(running).unwrap();

    registry.close();

    assert!(matches!(
        registry.register(job(JobState::Queued)),
        Err(RegisterError::Closed)
    ));
    assert_eq!(states(registry.list()), expected);
    assert_eq!(states(store.load().unwrap()), expected);
}

// Covers: an upgrade closes the registry only while no job is in progress,
// and not after a turn that ran since it found the daemon idle, which may
// have left work its other checks missed; once closed, no turn starts, so a
// prompt that races the upgrade is refused rather than interrupted (#1713).
// Owner: daemon job registry upgrade.
#[test]
fn an_upgrade_closes_the_registry_only_while_idle() {
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let job = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let id = job.id.clone();
    registry.register(job).unwrap();

    let before = registry.idle().unwrap();
    registry.start_turn(&id).unwrap();
    let while_running = registry.idle();
    registry
        .record(&id, JobEvent::TurnEnded(TurnEnd::Completed))
        .unwrap();
    // A turn that ran since the daemon was found idle may have left work.
    let after_a_turn = registry.close_if_still(before);
    let idle = registry.idle().unwrap();
    let still_idle = registry.close_if_still(idle);
    let next_turn = registry.start_turn(&id);

    assert_eq!(
        (while_running, after_a_turn, still_idle, next_turn),
        (None, false, true, Err(StartTurnError::Closed))
    );
}

// Covers: what a reopen finds for a session (#1661): nothing when no job ran
// its turns (another session's, or a finished subagent run that wrote it,
// does not count); the latest of its session jobs otherwise; and the job
// that writes it while one has a turn under way, a subagent run's included,
// so the session is not opened twice.
// Owner: daemon job registry session lookup.
#[test]
fn a_session_reopens_on_its_latest_job_unless_one_writes_it() {
    let session = rho_sdk::SessionId::new();
    let turns = |created_at, state| JobRecord {
        state,
        created_at,
        ..JobRecord::new_session(JobId::generate(), PathBuf::from("/work"), session.clone())
    };
    let subagent = |state| JobRecord {
        state,
        ..JobRecord::new_subagent(
            JobId::generate(),
            PathBuf::from("/work"),
            Some(session.clone()),
        )
    };
    let other = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let older = turns(1, JobState::Ended(TurnEnd::Completed));
    let newer = turns(2, JobState::Ended(TurnEnd::Interrupted));
    let running = turns(3, JobState::Running);
    let writing = subagent(JobState::Running);
    let wrote = subagent(JobState::Ended(TurnEnd::Completed));
    let cases = [
        ("nothing", vec![], SessionJob::New),
        ("another session", vec![other.clone()], SessionJob::New),
        ("a finished subagent", vec![wrote.clone()], SessionJob::New),
        (
            "two session jobs",
            vec![newer.clone(), older.clone(), wrote],
            SessionJob::Last(newer.clone()),
        ),
        (
            "a running turn",
            vec![older.clone(), running.clone()],
            SessionJob::Busy(running.id.clone()),
        ),
        (
            "a running subagent",
            vec![newer, other, writing.clone()],
            SessionJob::Busy(writing.id.clone()),
        ),
    ];
    for (name, records, expected) in cases {
        let home = TempDir::new().unwrap();
        let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
        for record in records {
            registry.register(record).unwrap();
        }
        assert_eq!(registry.session_job(&session), expected, "{name}");
    }
}
