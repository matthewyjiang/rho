use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use rho_providers::model::Message;
use rusqlite::{params, Connection};
use tempfile::TempDir;

use super::super::persistence::{
    session_dir_in_root, session_file_stats, workspace_key, SessionEntry,
};
use super::super::{SessionIndexRecord, SessionSummary};
use super::{
    apply_reconciliation_updates, indexed_files_for_scope, list_all_sessions,
    list_workspace_sessions, migrate_index, open_index, reconcile_sessions_with_hook,
    stale_index_keys, sync_workspace, IndexedFile, ReconcileScope,
};

#[test]
fn cold_workspace_sync_indexes_all_transcripts() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    write_test_session(root.path(), cwd.path(), "cold-a", 10, "first cold");
    write_test_session(root.path(), cwd.path(), "cold-b", 20, "second cold");

    sync_workspace(root.path(), cwd.path()).unwrap();

    let summaries = list_workspace_sessions(root.path(), cwd.path()).unwrap();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].id, "cold-b");
    assert_eq!(
        summaries[0].first_user_message.as_deref(),
        Some("second cold")
    );
    assert_eq!(summaries[1].id, "cold-a");
    assert_eq!(
        summaries[1].first_user_message.as_deref(),
        Some("first cold")
    );
}

#[test]
fn stale_workspace_sync_refreshes_changed_and_drops_missing() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let keep_path = write_test_session(root.path(), cwd.path(), "keep", 30, "keep me");
    let drop_path = write_test_session(root.path(), cwd.path(), "drop", 40, "drop me");
    sync_workspace(root.path(), cwd.path()).unwrap();

    // Rewrite keep so size/mtime force a stale re-summarize.
    fs::write(
        &keep_path,
        session_transcript_contents(cwd.path(), "keep", 30, "kept after edit"),
    )
    .unwrap();
    fs::remove_file(&drop_path).unwrap();

    sync_workspace(root.path(), cwd.path()).unwrap();
    let summaries = list_workspace_sessions(root.path(), cwd.path()).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "keep");
    assert_eq!(
        summaries[0].first_user_message.as_deref(),
        Some("kept after edit")
    );
}

// Covers: a transcript stored under one workspace cannot publish another
// workspace from its header into that workspace's index rows.
// Owner: session index reconciliation
#[test]
fn workspace_sync_rejects_misplaced_transcript() {
    let root = TempDir::new().unwrap();
    let owner = TempDir::new().unwrap();
    let foreign = TempDir::new().unwrap();
    let path = write_test_session(root.path(), owner.path(), "misplaced", 10, "prompt");
    fs::write(
        path,
        session_transcript_contents(foreign.path(), "misplaced", 10, "prompt"),
    )
    .unwrap();

    sync_workspace(root.path(), owner.path()).unwrap();

    assert_eq!(
        list_workspace_sessions(root.path(), owner.path()).unwrap(),
        []
    );
    assert_eq!(list_all_sessions(root.path()).unwrap(), []);
}

// Covers: a current-looking index row with a cwd that disagrees with its
// workspace key must be repaired from the owning transcript.
// Owner: session index reconciliation
#[test]
fn workspace_sync_repairs_mismatched_index_cwd() {
    let root = TempDir::new().unwrap();
    let owner = TempDir::new().unwrap();
    let foreign = TempDir::new().unwrap();
    write_test_session(root.path(), owner.path(), "mismatched", 10, "prompt");
    sync_workspace(root.path(), owner.path()).unwrap();

    {
        let connection = open_index(root.path()).unwrap();
        let connection = connection.lock().unwrap();
        connection
            .execute(
                "update sessions set cwd = ?1 where workspace_key = ?2 and id = ?3",
                params![
                    foreign.path().to_string_lossy().as_ref(),
                    workspace_key(owner.path()),
                    "mismatched"
                ],
            )
            .unwrap();
    }

    sync_workspace(root.path(), owner.path()).unwrap();

    let summaries = list_workspace_sessions(root.path(), owner.path()).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].cwd, owner.path());
}

// Covers: same-size header replacement with an unchanged mtime must invalidate
// a warm index row when the transcript now names another workspace.
// Owner: session index reconciliation
#[test]
fn warm_sync_rejects_header_cwd_change_with_unchanged_file_stats() {
    let root = TempDir::new().unwrap();
    let owner = TempDir::new().unwrap();
    let foreign = TempDir::new().unwrap();
    let path = write_test_session(root.path(), owner.path(), "warm-owner", 10, "prompt");
    sync_workspace(root.path(), owner.path()).unwrap();

    let original = fs::read(&path).unwrap();
    let replacement =
        session_transcript_contents(foreign.path(), "warm-owner", 10, "prompt").into_bytes();
    assert_eq!(replacement.len(), original.len());
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let original_stats = session_file_stats(&path);
    fs::write(&path, replacement).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(session_file_stats(&path), original_stats);

    sync_workspace(root.path(), owner.path()).unwrap();

    let connection = open_index(root.path()).unwrap();
    let connection = connection.lock().unwrap();
    let count: i64 = connection
        .query_row(
            "select count(*) from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key(owner.path()), "warm-owner"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

// Covers: a transcript and index row recreated between scan and commit must not
// be erased by a stale delete, and a competing malformed upsert is canonicalized.
// Owner: session index reconciliation
#[test]
fn reconciliation_revalidates_competing_index_writes() {
    let stale_root = TempDir::new().unwrap();
    let stale_owner = TempDir::new().unwrap();
    let stale_path = write_test_session(
        stale_root.path(),
        stale_owner.path(),
        "stale-race",
        10,
        "prompt",
    );
    sync_workspace(stale_root.path(), stale_owner.path()).unwrap();
    let stale_transcript = fs::read(&stale_path).unwrap();
    fs::remove_file(&stale_path).unwrap();
    let stale_key = workspace_key(stale_owner.path());
    let stale_scope = ReconcileScope::Workspace {
        workspace_key: stale_key.clone(),
        dir: session_dir_in_root(stale_root.path(), stale_owner.path()),
    };
    reconcile_sessions_with_hook(stale_root.path(), stale_scope, || {
        fs::write(&stale_path, stale_transcript)?;
        let competing = Connection::open(stale_root.path().join("index.sqlite3"))?;
        competing.execute(
            "update sessions set title = 'competing title'
             where workspace_key = ?1 and id = ?2",
            params![stale_key, "stale-race"],
        )?;
        Ok(())
    })
    .unwrap();
    let stale_connection = open_index(stale_root.path()).unwrap();
    let stale_connection = stale_connection.lock().unwrap();
    let stale_title: Option<String> = stale_connection
        .query_row(
            "select title from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key(stale_owner.path()), "stale-race"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale_title.as_deref(), Some("competing title"));
    drop(stale_connection);

    let upsert_root = TempDir::new().unwrap();
    let upsert_owner = TempDir::new().unwrap();
    let foreign = TempDir::new().unwrap();
    let upsert_path = write_test_session(
        upsert_root.path(),
        upsert_owner.path(),
        "upsert-race",
        10,
        "prompt",
    );
    let upsert_key = workspace_key(upsert_owner.path());
    let upsert_scope = ReconcileScope::Workspace {
        workspace_key: upsert_key.clone(),
        dir: session_dir_in_root(upsert_root.path(), upsert_owner.path()),
    };
    reconcile_sessions_with_hook(upsert_root.path(), upsert_scope, || {
        let competing = Connection::open(upsert_root.path().join("index.sqlite3"))?;
        competing.execute(
            "insert into sessions (
                workspace_key, cwd, id, path, created_at, updated_at, message_count
             ) values (?1, ?2, ?3, ?4, 1, 1, 0)",
            params![
                upsert_key,
                foreign.path().to_string_lossy().as_ref(),
                "upsert-race",
                upsert_path.to_string_lossy().as_ref()
            ],
        )?;
        Ok(())
    })
    .unwrap();
    let upsert_connection = open_index(upsert_root.path()).unwrap();
    let upsert_connection = upsert_connection.lock().unwrap();
    let stored_cwd: String = upsert_connection
        .query_row(
            "select cwd from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key(upsert_owner.path()), "upsert-race"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(Path::new(&stored_cwd), upsert_owner.path());
}

#[test]
fn workspace_updates_apply_atomically() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrate_index(&mut connection).unwrap();
    connection
        .execute_batch(
            "create trigger fail_second_insert before insert on sessions
             begin
               select raise(abort, 'forced failure')
               where (select count(*) from sessions) >= 1;
             end;",
        )
        .unwrap();

    let first = test_index_record("atomic-a", "/tmp/a.jsonl", "first");
    let second = test_index_record("atomic-b", "/tmp/b.jsonl", "second");
    let records = vec![
        (("ws".into(), "atomic-a".into()), first),
        (("ws".into(), "atomic-b".into()), second),
    ];
    let error = apply_reconciliation_updates(&mut connection, &records, &[]).unwrap_err();
    assert!(error.to_string().contains("forced failure"));

    let count: i64 = connection
        .query_row("select count(*) from sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "failed batch must roll back every upsert");
}

#[test]
fn workspace_updates_delete_stale_ids_in_same_transaction() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrate_index(&mut connection).unwrap();
    let keep = test_index_record("keep", "/tmp/keep.jsonl", "keep");
    let drop = test_index_record("drop", "/tmp/drop.jsonl", "drop");
    let records = vec![
        (("ws".into(), "keep".into()), keep),
        (("ws".into(), "drop".into()), drop),
    ];
    apply_reconciliation_updates(&mut connection, &records, &[]).unwrap();

    let refreshed = test_index_record("keep", "/tmp/keep.jsonl", "kept fresh");
    apply_reconciliation_updates(
        &mut connection,
        &[(("ws".into(), "keep".into()), refreshed)],
        &[("ws".into(), "drop".into())],
    )
    .unwrap();

    let rows = connection
        .prepare("select id, first_user_message from sessions order by id")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![("keep".into(), Some("kept fresh".into()))],
        "upserts and stale deletes must land together"
    );
}

#[test]
fn stale_index_keys_uses_loaded_map_without_requery() {
    let existing_dir = TempDir::new().unwrap();
    let existing_path = existing_dir.path().join("exists.jsonl");
    fs::write(&existing_path, "").unwrap();
    let indexed = HashMap::from([
        (
            ("ws".into(), "seen-current".into()),
            IndexedFile {
                cwd: "/tmp".into(),
                path: existing_path.to_string_lossy().into_owned(),
                file_size: Some(1),
                file_mtime: Some(1),
                message_count: 1,
                first_user_message: Some("x".into()),
            },
        ),
        (
            ("ws".into(), "seen-path-moved".into()),
            IndexedFile {
                cwd: "/tmp".into(),
                path: "/tmp/definitely-missing-rho-session.jsonl".into(),
                file_size: Some(1),
                file_mtime: Some(1),
                message_count: 0,
                first_user_message: None,
            },
        ),
        (
            ("ws".into(), "seen-unreadable".into()),
            IndexedFile {
                cwd: "/tmp".into(),
                path: "/tmp/definitely-missing-rho-session-unreadable.jsonl".into(),
                file_size: Some(1),
                file_mtime: Some(1),
                message_count: 0,
                first_user_message: None,
            },
        ),
        (
            ("ws".into(), "not-seen".into()),
            IndexedFile {
                cwd: "/tmp".into(),
                path: existing_path.to_string_lossy().into_owned(),
                file_size: Some(1),
                file_mtime: Some(1),
                message_count: 0,
                first_user_message: None,
            },
        ),
    ]);
    let seen = HashSet::from([
        ("ws".into(), "seen-current".into()),
        ("ws".into(), "seen-path-moved".into()),
        ("ws".into(), "seen-unreadable".into()),
    ]);
    let refreshed = HashSet::from([("ws".into(), "seen-path-moved".into())]);
    let mut stale = stale_index_keys(&indexed, &seen, &refreshed);
    stale.sort();
    assert_eq!(
        stale,
        vec![
            ("ws".to_string(), "not-seen".to_string()),
            ("ws".to_string(), "seen-unreadable".to_string()),
        ]
    );
}

#[test]
fn warm_sync_skips_current_files_using_loaded_map() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    write_test_session(root.path(), cwd.path(), "warm", 50, "warm prompt");
    sync_workspace(root.path(), cwd.path()).unwrap();

    let connection = open_index(root.path()).unwrap();
    let connection = connection.lock().unwrap();
    let workspace_key = workspace_key(cwd.path());
    let scope = ReconcileScope::Workspace {
        workspace_key: workspace_key.clone(),
        dir: session_dir_in_root(root.path(), cwd.path()),
    };
    let indexed = indexed_files_for_scope(&connection, &scope).unwrap();
    let warm = indexed
        .get(&(workspace_key, "warm".into()))
        .expect("warm row");
    let path = PathBuf::from(&warm.path);
    let (size, mtime) = session_file_stats(&path);
    assert!(
        warm.is_current(&path, size, mtime),
        "freshly synced row must compare current against the loaded map"
    );
}

// Covers: cross-project list must index never-synced workspace dirs
// Owner: session index
#[test]
fn list_all_sessions_discovers_unindexed_workspaces() {
    let root = TempDir::new().unwrap();
    let cwd_a = TempDir::new().unwrap();
    let cwd_b = TempDir::new().unwrap();
    write_test_session(root.path(), cwd_a.path(), "a-session", 10, "from a");
    write_test_session(root.path(), cwd_b.path(), "b-session", 20, "from b");

    let all = list_all_sessions(root.path()).unwrap();
    let ids = all
        .iter()
        .map(|summary| summary.id.as_str())
        .collect::<HashSet<_>>();
    assert!(ids.contains("a-session"));
    assert!(ids.contains("b-session"));
    assert_eq!(all.len(), 2);
}

// Covers: bulk reconcile must drop index rows whose files are gone
// Owner: session index
#[test]
fn list_all_sessions_drops_missing_files() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let path = write_test_session(root.path(), cwd.path(), "gone", 30, "delete me");
    assert_eq!(list_all_sessions(root.path()).unwrap().len(), 1);

    fs::remove_file(&path).unwrap();
    let all = list_all_sessions(root.path()).unwrap();
    assert!(all.is_empty());
}

// Covers: global listing must return one canonical summary when two index rows
// point at one physical session and reconcile cannot repair the stale row.
// Owner: session index
#[test]
fn list_all_sessions_canonicalizes_duplicate_physical_path() {
    let root = TempDir::new().unwrap();
    let canonical_cwd = TempDir::new().unwrap();
    let id = "shared-session";
    let path = write_test_session(root.path(), canonical_cwd.path(), id, 30, "canonical");
    assert_eq!(list_all_sessions(root.path()).unwrap().len(), 1);

    let stale_cwd = root.path().join("missing-cwd");
    let stale_workspace_key = workspace_key(&stale_cwd);
    let stale_dir = session_dir_in_root(root.path(), &stale_cwd);
    fs::create_dir_all(&stale_dir).unwrap();
    fs::write(stale_dir.join(format!("10_{id}.jsonl")), "not json").unwrap();
    {
        let connection = open_index(root.path()).unwrap();
        let connection = connection.lock().unwrap();
        connection
            .execute(
                "insert into sessions (
                    workspace_key, cwd, id, path, created_at, updated_at, message_count
                 ) values (?1, ?2, ?3, ?4, 1, 10, 0)",
                params![
                    stale_workspace_key,
                    stale_cwd.to_string_lossy().as_ref(),
                    id,
                    path.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
    }

    let all = list_all_sessions(root.path()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].path, path);
    assert_eq!(all[0].cwd, canonical_cwd.path());

    let connection = open_index(root.path()).unwrap();
    let connection = connection.lock().unwrap();
    let row_count: i64 = connection
        .query_row("select count(*) from sessions where id = ?1", [id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(row_count, 1, "listing must remove non-canonical rows");
}

// Covers: global reconcile must remove a missing workspace row even when its
// stored path points at a live session owned by another workspace.
// Owner: session index
#[test]
fn list_all_sessions_drops_absent_workspace_row_for_live_path() {
    let root = TempDir::new().unwrap();
    let canonical_cwd = TempDir::new().unwrap();
    let id = "shared-session";
    let path = write_test_session(root.path(), canonical_cwd.path(), id, 30, "canonical");
    assert_eq!(list_all_sessions(root.path()).unwrap().len(), 1);

    let stale_cwd = root.path().join("missing-cwd");
    let stale_workspace_key = workspace_key(&stale_cwd);
    {
        let connection = open_index(root.path()).unwrap();
        let connection = connection.lock().unwrap();
        connection
            .execute(
                "insert into sessions (
                    workspace_key, cwd, id, path, created_at, updated_at, message_count
                 ) values (?1, ?2, ?3, ?4, 1, 999, 0)",
                params![
                    stale_workspace_key,
                    stale_cwd.to_string_lossy().as_ref(),
                    id,
                    path.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
    }

    let all = list_all_sessions(root.path()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].path, path);
    assert_eq!(all[0].cwd, canonical_cwd.path());

    let connection = open_index(root.path()).unwrap();
    let connection = connection.lock().unwrap();
    let stale_count: i64 = connection
        .query_row(
            "select count(*) from sessions where workspace_key = ?1 and id = ?2",
            params![stale_workspace_key, id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale_count, 0);
}

fn write_test_session(
    session_root: &Path,
    cwd: &Path,
    id: &str,
    created_at: u64,
    prompt: &str,
) -> PathBuf {
    let dir = session_dir_in_root(session_root, cwd);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{created_at}_{id}.jsonl"));
    fs::write(
        &path,
        session_transcript_contents(cwd, id, created_at, prompt),
    )
    .unwrap();
    path
}

fn session_transcript_contents(cwd: &Path, id: &str, created_at: u64, prompt: &str) -> String {
    let header = SessionEntry::Session {
        version: 3,
        id: id.into(),
        timestamp: created_at.to_string(),
        cwd: cwd.to_path_buf(),
        agent_id: None,
        agent_fingerprint: None,
    };
    let message = SessionEntry::Message {
        timestamp: created_at.to_string(),
        message: Message::user_text(prompt),
        display_message: None,
    };
    format!(
        "{}\n{}\n",
        serde_json::to_string(&header).unwrap(),
        serde_json::to_string(&message).unwrap()
    )
}

fn test_index_record(id: &str, path: &str, first_user: &str) -> SessionIndexRecord {
    SessionIndexRecord {
        summary: SessionSummary {
            id: id.into(),
            path: PathBuf::from(path),
            cwd: PathBuf::from("/tmp/ws"),
            created_at: 1,
            updated_at: 1,
            message_count: 1,
            title: None,
            first_user_message: Some(first_user.into()),
            last_user_message: Some(first_user.into()),
        },
        file_size: Some(10),
        file_mtime: Some(10),
        node_count: 0,
        branch_count: 0,
        active_leaf_id: None,
        effective_format_version: 3,
    }
}
