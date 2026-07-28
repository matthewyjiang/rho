use std::{
    fs,
    sync::{mpsc, Arc, Barrier},
    thread,
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{
    index::{
        initialize_index, insert_parent_lock_for_test, unix_timestamp_secs, PARENT_LOCK_TTL_SECS,
    },
    lock_parent_for_cleanup_in_root, reserve_run_directory_in_root, resolve_run_directory_in_root,
    RunPlacement,
};
use crate::session::Session;
use std::path::{Path, PathBuf};

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

    let (id, directory) = reserve_run_directory_in_root(
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
        reserve_run_directory_in_root(temp.path(), &placement, || "abcdef".into()).unwrap_err();

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
        reserve_run_directory_in_root(
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

    let (_, directory) = reserve_run_directory_in_root(
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

    let (_, directory) = reserve_run_directory_in_root(
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
    reserve_run_directory_in_root(temp.path(), &placement, || "abcdef".into()).unwrap();
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

    let (_, directory) = reserve_run_directory_in_root(
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
