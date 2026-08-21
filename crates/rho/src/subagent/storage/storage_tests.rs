use std::{
    fs,
    sync::{mpsc, Arc, Barrier},
    thread,
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::paths::{prepare_private_directory, SessionSubagentsDir};
use super::{
    index::{
        initialize_index, insert_parent_lock_for_test, unix_timestamp_secs, PARENT_LOCK_TTL_SECS,
    },
    is_trusted_directory, list_workspace_runs_in_root, lock_parent_for_cleanup_in_root,
    reserve_run_directory_in_root as reserve_at, resolve_run_directory_in_root, RunPlacement,
};
use crate::session::Session;
use std::path::{Path, PathBuf};

fn reserve_in_default_workspace(
    rho_root: &Path,
    placement: &RunPlacement,
    next_id: impl FnMut() -> String,
) -> anyhow::Result<(String, PathBuf)> {
    reserve_at(rho_root, rho_root, placement, next_id)
}

fn create_session_subagents(root: &Path) -> PathBuf {
    let cwd = TempDir::new().unwrap();
    Session::create_in_root(&root.join("sessions"), cwd.path())
        .unwrap()
        .subagents_dir()
        .unwrap()
}

#[test]
fn skips_ids_used_by_unindexed_target_path() {
    let temp = TempDir::new().unwrap();
    let existing = temp.path().join("subagents/111111");
    fs::create_dir_all(&existing).unwrap();
    let mut ids = ["111111", "222222"].into_iter();

    let (id, directory) = reserve_in_default_workspace(
        temp.path(),
        &RunPlacement::Global {
            parent_session_id: None,
        },
        || ids.next().unwrap().into(),
    )
    .unwrap();

    assert_eq!(id, "222222");
    assert_eq!(directory, temp.path().join("subagents/222222"));
}

#[test]
fn scan_fallback_resolves_one_nested_run() {
    let temp = TempDir::new().unwrap();
    let directory = create_session_subagents(temp.path()).join("123abc");
    fs::create_dir_all(&directory).unwrap();

    assert_eq!(
        resolve_run_directory_in_root(temp.path(), "123abc").unwrap(),
        directory
    );
}

#[test]
fn scan_fallback_reports_ambiguous_nested_runs() {
    let temp = TempDir::new().unwrap();
    for _ in 0..2 {
        fs::create_dir_all(create_session_subagents(temp.path()).join("123abc")).unwrap();
    }

    let error = resolve_run_directory_in_root(temp.path(), "123abc").unwrap_err();
    assert!(error
        .to_string()
        .contains("ambiguous across session folders"));
}

#[test]
fn legacy_global_fallback_resolves_without_index() {
    let temp = TempDir::new().unwrap();
    let directory = temp.path().join("subagents/654321");
    fs::create_dir_all(&directory).unwrap();

    assert_eq!(
        resolve_run_directory_in_root(temp.path(), "654321").unwrap(),
        directory
    );
}

#[test]
fn concurrent_index_initialization_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("subagents/index.sqlite3");
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                initialize_index(&path).map(drop)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
}

// Covers: concurrent first reservations must both succeed while racing to
// create a missing session subagents directory — the loser used to fail with
// EEXIST ("File exists (os error 17)") because the pre-fix code only
// tolerated a directory that already existed at check time.
// Owner: delegated-run reservation
#[test]
fn concurrent_session_subagents_dir_creation_is_tolerated() {
    for _ in 0..16 {
        let temp = TempDir::new().unwrap();
        let subagents = temp.path().join("sessions/ws/session-1/subagents");
        fs::create_dir_all(subagents.parent().unwrap()).unwrap();
        let dir = SessionSubagentsDir::parse(temp.path(), &subagents).unwrap();
        assert!(!subagents.exists());

        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let dir = dir.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    dir.ensure_ready()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        assert!(is_trusted_directory(&subagents));
    }
}

// Covers: a reservation must succeed when the session subagents directory
// already exists — the loser of a concurrent first-spawn create used to fail
// here with EEXIST ("File exists (os error 17)").
// Owner: delegated-run reservation
#[test]
fn reservations_tolerate_existing_session_subagents_dir() {
    let temp = TempDir::new().unwrap();
    // Simulate the winner of a concurrent first-spawn create: the directory
    // exists by the time this reservation checks.
    let subagents_dir = create_session_subagents(temp.path());
    fs::create_dir(&subagents_dir).unwrap();
    let placement = RunPlacement::Session {
        parent_session_id: "session-id".into(),
        subagents_dir: subagents_dir.clone(),
    };

    let (id, directory) =
        reserve_in_default_workspace(temp.path(), &placement, || "abcdef".into()).unwrap();

    assert_eq!(id, "abcdef");
    assert_eq!(directory, subagents_dir.join("abcdef"));
}

// Covers: private-directory preparation must create a missing leaf with
// private modes and stay idempotent when the leaf already exists.
// Owner: delegated-run reservation
#[test]
fn prepare_private_directory_creates_and_secures_missing_leaf() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("nested/subagents");

    prepare_private_directory(&target).unwrap();
    prepare_private_directory(&target).unwrap();

    assert!(is_trusted_directory(&target));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

// Covers: a non-directory at the target path must still be rejected even
// though the create step tolerates AlreadyExists from concurrent creators.
// Owner: delegated-run reservation
#[test]
fn prepare_private_directory_rejects_non_directory_target() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("not-a-dir");
    fs::write(&target, b"x").unwrap();

    let error = prepare_private_directory(&target).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn reservation_fails_after_parent_session_is_deleted() {
    let temp = TempDir::new().unwrap();
    let subagents_dir = create_session_subagents(temp.path());
    fs::remove_dir_all(subagents_dir.parent().unwrap()).unwrap();
    let placement = RunPlacement::Session {
        parent_session_id: "deleted-session".into(),
        subagents_dir: subagents_dir.clone(),
    };

    let error =
        reserve_in_default_workspace(temp.path(), &placement, || "abcdef".into()).unwrap_err();

    assert!(error
        .to_string()
        .contains("not a trusted session directory"));
    assert!(!subagents_dir.exists());
}

#[test]
fn parent_cleanup_lock_blocks_reservations_for_that_parent() {
    let temp = TempDir::new().unwrap();
    let rho_root = temp.path().to_path_buf();
    let subagents_root = rho_root.join("subagents");
    let session_subagents = create_session_subagents(&rho_root);
    let session_dir = session_subagents.parent().unwrap().to_path_buf();
    let deleted_session_dir = session_dir.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cleanup_root = subagents_root.clone();
    let cleanup = thread::spawn(move || {
        let guard = lock_parent_for_cleanup_in_root(&cleanup_root, "session-id")?;
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        fs::remove_dir_all(session_dir)?;
        guard.clear_index_and_unlock()
    });
    entered_rx.recv().unwrap();

    let (reserve_started_tx, reserve_started_rx) = mpsc::channel();
    let reserve_root = rho_root.clone();
    let reserve = thread::spawn(move || {
        reserve_in_default_workspace(
            &reserve_root,
            &RunPlacement::Session {
                parent_session_id: "session-id".into(),
                subagents_dir: session_subagents,
            },
            || {
                reserve_started_tx.send(()).unwrap();
                "abcdef".into()
            },
        )
    });
    reserve_started_rx.recv().unwrap();
    let reserve_error = reserve.join().unwrap().unwrap_err();
    assert!(
        reserve_error.to_string().contains("is being deleted"),
        "{reserve_error}"
    );
    release_tx.send(()).unwrap();

    cleanup.join().unwrap().unwrap();
    assert!(!deleted_session_dir.exists());
}

#[test]
fn parent_cleanup_lock_does_not_block_unrelated_parents() {
    let temp = TempDir::new().unwrap();
    let rho_root = temp.path();
    let subagents_root = rho_root.join("subagents");
    let other_subagents = create_session_subagents(rho_root);
    let _guard = lock_parent_for_cleanup_in_root(&subagents_root, "deleting-session").unwrap();

    let (_, directory) = reserve_in_default_workspace(
        rho_root,
        &RunPlacement::Session {
            parent_session_id: "other-session".into(),
            subagents_dir: other_subagents.clone(),
        },
        || "abcdef".into(),
    )
    .unwrap();

    assert_eq!(directory, other_subagents.join("abcdef"));
}

#[test]
fn stale_parent_lock_is_ignored_by_reserve() {
    let temp = TempDir::new().unwrap();
    let subagents_root = temp.path().join("subagents");
    let subagents_dir = create_session_subagents(temp.path());
    let stale_at = unix_timestamp_secs() - PARENT_LOCK_TTL_SECS - 1;
    insert_parent_lock_for_test(&subagents_root, "session-id", stale_at).unwrap();

    let (_, directory) = reserve_in_default_workspace(
        temp.path(),
        &RunPlacement::Session {
            parent_session_id: "session-id".into(),
            subagents_dir: subagents_dir.clone(),
        },
        || "abcdef".into(),
    )
    .unwrap();

    assert_eq!(directory, subagents_dir.join("abcdef"));
}

#[test]
fn stale_parent_lock_can_be_stolen_for_cleanup() {
    let temp = TempDir::new().unwrap();
    let subagents_root = temp.path().join("subagents");
    let stale_at = unix_timestamp_secs() - PARENT_LOCK_TTL_SECS - 1;
    insert_parent_lock_for_test(&subagents_root, "session-id", stale_at).unwrap();

    let guard = lock_parent_for_cleanup_in_root(&subagents_root, "session-id").unwrap();
    guard.clear_index_and_unlock().unwrap();

    // A fresh lock should succeed after the stolen cleanup finished.
    let _guard = lock_parent_for_cleanup_in_root(&subagents_root, "session-id").unwrap();
}

#[test]
fn fresh_parent_lock_rejects_second_cleanup() {
    let temp = TempDir::new().unwrap();
    let subagents_root = temp.path().join("subagents");
    let _guard = lock_parent_for_cleanup_in_root(&subagents_root, "session-id").unwrap();
    let error = lock_parent_for_cleanup_in_root(&subagents_root, "session-id").unwrap_err();
    assert!(
        error.to_string().contains("already being deleted"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn scan_ignores_symlinked_session_ancestors() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let external_run = outside.path().join("session/subagents/abcdef");
    fs::create_dir_all(&external_run).unwrap();
    let sessions_root = temp.path().join("sessions");
    fs::create_dir_all(&sessions_root).unwrap();
    symlink(outside.path(), sessions_root.join("linked-workspace")).unwrap();

    let error = resolve_run_directory_in_root(temp.path(), "abcdef").unwrap_err();
    assert!(error.to_string().contains("unknown delegated run"));
}

#[test]
fn stale_index_row_falls_through_to_legacy_global() {
    let temp = TempDir::new().unwrap();
    let indexed = create_session_subagents(temp.path()).join("abcdef");
    let placement = RunPlacement::Session {
        parent_session_id: "session".into(),
        subagents_dir: indexed.parent().unwrap().to_path_buf(),
    };
    reserve_in_default_workspace(temp.path(), &placement, || "abcdef".into()).unwrap();
    fs::remove_dir(&indexed).unwrap();
    let legacy = temp.path().join("subagents/abcdef");
    fs::create_dir_all(&legacy).unwrap();

    assert_eq!(
        resolve_run_directory_in_root(temp.path(), "abcdef").unwrap(),
        legacy
    );
}

#[test]
fn failed_cleanup_releases_parent_lock() {
    let temp = TempDir::new().unwrap();
    let subagents_root = temp.path().join("subagents");
    let subagents_dir = create_session_subagents(temp.path());

    {
        let _guard = lock_parent_for_cleanup_in_root(&subagents_root, "session-id").unwrap();
    }

    let (_, directory) = reserve_in_default_workspace(
        temp.path(),
        &RunPlacement::Session {
            parent_session_id: "session-id".into(),
            subagents_dir: subagents_dir.clone(),
        },
        || "abcdef".into(),
    )
    .unwrap();
    assert_eq!(directory, subagents_dir.join("abcdef"));
}

// Covers: rho attach picker must not list subagents from another directory.
// Owner: delegated-run index listing
#[test]
fn list_running_runs_keeps_only_the_current_workspace() {
    let temp = TempDir::new().unwrap();
    let here = TempDir::new().unwrap();
    let there = TempDir::new().unwrap();
    let here_session = Session::create_in_root(&temp.path().join("sessions"), here.path()).unwrap();
    let there_session =
        Session::create_in_root(&temp.path().join("sessions"), there.path()).unwrap();
    let here_nested = reserve_running(
        temp.path(),
        here.path(),
        RunPlacement::Session {
            parent_session_id: here_session.id().to_string(),
            subagents_dir: here_session.subagents_dir().unwrap(),
        },
        "aaaaaa",
        "here-worker",
    );
    let there_nested = reserve_running(
        temp.path(),
        there.path(),
        RunPlacement::Session {
            parent_session_id: there_session.id().to_string(),
            subagents_dir: there_session.subagents_dir().unwrap(),
        },
        "bbbbbb",
        "there-worker",
    );
    let here_parentless = reserve_running(
        temp.path(),
        here.path(),
        RunPlacement::Global {
            parent_session_id: None,
        },
        "cccccc",
        "here-orphan",
    );
    let there_parentless = reserve_running(
        temp.path(),
        there.path(),
        RunPlacement::Global {
            parent_session_id: None,
        },
        "dddddd",
        "there-orphan",
    );

    let here_ids = workspace_ids(temp.path(), here.path());
    let there_ids = workspace_ids(temp.path(), there.path());

    assert_eq!(
        here_ids,
        std::collections::BTreeSet::from([here_nested, here_parentless])
    );
    assert_eq!(
        there_ids,
        std::collections::BTreeSet::from([there_nested, there_parentless])
    );
    assert_eq!(
        workspace_ids(temp.path(), &here.path().canonicalize().unwrap()),
        here_ids
    );
}

// Covers: attach listing can include finished transcripts for the same workspace.
// Owner: delegated-run index listing
#[test]
fn list_workspace_runs_can_include_finished_runs() {
    let temp = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let running = reserve_running(
        temp.path(),
        cwd.path(),
        RunPlacement::parentless(),
        "aaaaaa",
        "worker",
    );
    let finished = reserve_running(
        temp.path(),
        cwd.path(),
        RunPlacement::parentless(),
        "bbbbbb",
        "explorer",
    );
    write_terminal(temp.path(), &finished, crate::subagent::RunState::Ok);

    assert_eq!(
        workspace_ids(temp.path(), cwd.path()),
        std::collections::BTreeSet::from([running, finished])
    );
}

// Covers: a v3 index must upgrade and keep unscoped rows out of the picker.
// Owner: delegated-run index listing
#[test]
fn v3_index_upgrades_and_hides_unscoped_rows_from_the_picker() {
    let temp = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let directory = temp.path().join("subagents/eeeeee");
    fs::create_dir_all(&directory).unwrap();
    crate::subagent::write_status(
        &directory.join(crate::subagent::RESULT_FILE_NAME),
        &crate::subagent::RunStatus {
            state: crate::subagent::RunState::Running,
            agent_id: Some("legacy".into()),
            started_at: Some(1),
            ..crate::subagent::RunStatus::default()
        },
    )
    .unwrap();

    let index_path = temp.path().join("subagents/index.sqlite3");
    let connection = rusqlite::Connection::open(&index_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE runs (
                 run_id TEXT PRIMARY KEY NOT NULL,
                 path TEXT NOT NULL UNIQUE,
                 parent_session_id TEXT,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE parent_locks (
                 parent_session_id TEXT PRIMARY KEY NOT NULL,
                 locked_at INTEGER NOT NULL
             );
             PRAGMA user_version = 3;",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (run_id, path, parent_session_id, created_at)
             VALUES (?1, ?2, NULL, ?3)",
            rusqlite::params!["eeeeee", directory.to_string_lossy(), unix_timestamp_secs()],
        )
        .unwrap();
    drop(connection);

    assert!(workspace_ids(temp.path(), cwd.path()).is_empty());
    assert_eq!(
        resolve_run_directory_in_root(temp.path(), "eeeeee").unwrap(),
        directory
    );
}

fn reserve_running(
    rho_root: &Path,
    cwd: &Path,
    placement: RunPlacement,
    id: &str,
    agent_id: &str,
) -> String {
    let next_id = id.to_string();
    let (id, directory) = reserve_at(rho_root, cwd, &placement, move || next_id.clone()).unwrap();
    crate::subagent::write_status(
        &directory.join(crate::subagent::RESULT_FILE_NAME),
        &crate::subagent::RunStatus {
            state: crate::subagent::RunState::Running,
            agent_id: Some(agent_id.into()),
            started_at: Some(1),
            ..crate::subagent::RunStatus::default()
        },
    )
    .unwrap();
    id
}

fn write_terminal(rho_root: &Path, id: &str, state: crate::subagent::RunState) {
    let directory = resolve_run_directory_in_root(rho_root, id).unwrap();
    crate::subagent::write_status(
        &directory.join(crate::subagent::RESULT_FILE_NAME),
        &crate::subagent::RunStatus {
            state,
            agent_id: Some("explorer".into()),
            started_at: Some(1),
            finished_at: Some(2),
            ..crate::subagent::RunStatus::default()
        },
    )
    .unwrap();
}

fn workspace_ids(rho_root: &Path, cwd: &Path) -> std::collections::BTreeSet<String> {
    list_workspace_runs_in_root(rho_root, cwd)
        .unwrap()
        .into_iter()
        .map(|run| run.id)
        .collect()
}
