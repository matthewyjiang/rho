use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::JobStore;
use crate::app::daemon_host::jobs::{JobId, JobRecord, JobState, TurnEnd};

fn record(state: JobState) -> JobRecord {
    JobRecord {
        state,
        ..JobRecord::new_session(PathBuf::from("/work/workspace"), rho_sdk::SessionId::new())
    }
}

fn sorted(mut records: Vec<JobRecord>) -> Vec<JobRecord> {
    records.sort_by(|a, b| a.id.cmp(&b.id));
    records
}

// Covers: a record written and loaded back is the same record, rewriting a
// record replaces it rather than adding a second, and the directory is
// private.
// Owner: daemon job store.
#[test]
fn records_round_trip_and_rewrites_replace_them() {
    let home = TempDir::new().unwrap();
    let store = JobStore::open(home.path()).unwrap();
    let failed = record(JobState::Ended(TurnEnd::Failed(
        "provider refused the request".into(),
    )));
    let mut running = record(JobState::Running);
    store.write(&failed).unwrap();
    store.write(&running).unwrap();
    running.state = JobState::WaitingForInput;
    store.write(&running).unwrap();

    assert_eq!(sorted(store.load().unwrap()), sorted(vec![failed, running]));
    let mode = fs::metadata(home.path().join("jobs"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

// Covers: files that are not a record named for its own id (not JSON, not a
// record, named for another job) are skipped without hiding the valid
// records, and files that are not record files at all (an interrupted
// write's temp file, a stray note) are ignored; a new store holds no
// records.
// Owner: daemon job store.
#[test]
fn unreadable_record_files_are_skipped() {
    let home = TempDir::new().unwrap();
    let store = JobStore::open(home.path()).unwrap();
    assert_eq!(store.load().unwrap(), Vec::new());

    let kept = record(JobState::Ended(TurnEnd::Completed));
    store.write(&kept).unwrap();
    let dir = home.path().join("jobs");
    let misnamed = serde_json::to_vec(&record(JobState::Idle)).unwrap();
    for (name, contents) in [
        (
            format!("{}.json", JobId::generate()),
            b"{ not json".to_vec(),
        ),
        (
            format!("{}.json", JobId::generate()),
            b"{\"id\": 3}".to_vec(),
        ),
        (format!("{}.json", JobId::generate()), misnamed),
        ("not-a-job.json".to_owned(), b"{}".to_vec()),
        (format!(".{}.json.0123.tmp", kept.id), b"{".to_vec()),
        ("notes.txt".to_owned(), b"hello".to_vec()),
    ] {
        fs::write(dir.join(name), contents).unwrap();
    }

    assert_eq!(store.load().unwrap(), vec![kept]);
}
