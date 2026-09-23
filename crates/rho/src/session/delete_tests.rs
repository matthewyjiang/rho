use std::path::Path;

use pretty_assertions::assert_eq;

use super::{delete_target_in_roots, delete_targets_in_roots};
use crate::session::{DeleteOptions, Session, SessionTarget};

fn create_session(root: &Path, cwd: &Path, id: &str) -> Session {
    Session::create_with_id_in_root(root, cwd, id, None).unwrap()
}

// Covers: an exact workspace target cannot fall back to a same-id session in
// another workspace during delete.
// Owner: session deletion
#[test]
fn exact_target_delete_preserves_same_id_in_other_workspace() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace_a = tempfile::tempdir().unwrap();
    let workspace_b = tempfile::tempdir().unwrap();
    let id = "00000000-0000-0000-0000-000000000123";
    let kept = create_session(sessions.path(), workspace_a.path(), id);
    let deleted = create_session(sessions.path(), workspace_b.path(), id);
    let deleted_path = deleted.path().to_path_buf();
    drop(deleted);
    let outcome = delete_target_in_roots(
        sessions.path(),
        subagents.path(),
        &SessionTarget::new(id, workspace_b.path()),
        &DeleteOptions::default(),
    )
    .unwrap();

    assert_eq!(outcome.cwd, workspace_b.path());
    assert!(kept.path().exists());
    assert!(!deleted_path.exists());
}

// Covers: workspace batch deletion owns current-session protection and deletes
// every other persisted session.
// Owner: session deletion
#[test]
fn workspace_delete_keeps_exact_protected_session() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let kept_id = "00000000-0000-0000-0000-000000000124";
    let deleted_id = "00000000-0000-0000-0000-000000000125";
    let kept = create_session(sessions.path(), workspace.path(), kept_id);
    let deleted = create_session(sessions.path(), workspace.path(), deleted_id);
    let deleted_path = deleted.path().to_path_buf();
    drop(deleted);
    let protected = SessionTarget::new(kept_id, workspace.path());

    let outcome = delete_targets_in_roots(
        sessions.path(),
        subagents.path(),
        &[
            protected.clone(),
            SessionTarget::new(deleted_id, workspace.path()),
        ],
        &DeleteOptions {
            force: false,
            protected_session: Some(protected.clone()),
        },
    )
    .unwrap();

    assert_eq!(outcome.kept_protected, vec![protected]);
    assert_eq!(
        outcome
            .deleted
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec![deleted_id]
    );
    assert!(outcome.failures.is_empty());
    assert!(kept.path().exists());
    assert!(!deleted_path.exists());
}

// Covers: a second process cannot delete a transcript while its owning session
// holds the durable active lease.
// Owner: session deletion
#[test]
fn active_session_lease_blocks_delete_until_session_closes() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let id = "00000000-0000-0000-0000-000000000126";
    let session = create_session(sessions.path(), workspace.path(), id);
    let target = SessionTarget::new(id, workspace.path());

    assert!(delete_target_in_roots(
        sessions.path(),
        subagents.path(),
        &target,
        &DeleteOptions::default(),
    )
    .is_err());
    assert!(session.path().exists());

    let path = session.path().to_path_buf();
    drop(session);
    delete_target_in_roots(
        sessions.path(),
        subagents.path(),
        &target,
        &DeleteOptions::default(),
    )
    .unwrap();
    assert!(!path.exists());
}

// Covers: batch deletion removes only targets reviewed before confirmation, so
// a session created between preview and confirmation survives.
// Owner: session deletion
#[test]
fn target_batch_does_not_delete_session_created_after_preview() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let reviewed_id = "00000000-0000-0000-0000-000000000127";
    let later_id = "00000000-0000-0000-0000-000000000128";
    let reviewed = create_session(sessions.path(), workspace.path(), reviewed_id);
    let reviewed_path = reviewed.path().to_path_buf();
    let reviewed_targets = vec![SessionTarget::new(reviewed_id, workspace.path())];
    drop(reviewed);

    let later = create_session(sessions.path(), workspace.path(), later_id);
    let later_path = later.path().to_path_buf();
    drop(later);

    let outcome = delete_targets_in_roots(
        sessions.path(),
        subagents.path(),
        &reviewed_targets,
        &DeleteOptions::default(),
    )
    .unwrap();

    assert_eq!(outcome.deleted.len(), 1);
    assert!(!reviewed_path.exists());
    assert!(later_path.exists());
}

// Covers: a batch keeps going past a session it cannot delete, and drops the
// index rows of the ones it did delete even though that happens at batch end.
// Owner: session deletion
#[test]
fn batch_delete_skips_blocked_session_and_drops_deleted_index_rows() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let ids = [
        "00000000-0000-0000-0000-000000000131",
        "00000000-0000-0000-0000-000000000132",
        "00000000-0000-0000-0000-000000000133",
    ];
    for id in &ids {
        drop(create_session(sessions.path(), workspace.path(), id));
    }
    // An open session holds its active lease, which blocks its delete.
    let blocked = Session::open_by_id_in_root(sessions.path(), workspace.path(), ids[1])
        .unwrap()
        .0;

    let outcome = delete_targets_in_roots(
        sessions.path(),
        subagents.path(),
        &ids.map(|id| SessionTarget::new(id, workspace.path())),
        &DeleteOptions::default(),
    )
    .unwrap();

    let deleted = outcome
        .deleted
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    let failed = outcome
        .failures
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!((deleted, failed), (vec![ids[0], ids[2]], vec![ids[1]]));
    drop(blocked);
    // Read the index directly: listing would reconcile leftover rows away.
    let index = rusqlite::Connection::open(sessions.path().join("index.sqlite3")).unwrap();
    let indexed = index
        .prepare("select id from sessions order by id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(indexed, vec![ids[1].to_string()]);
}
