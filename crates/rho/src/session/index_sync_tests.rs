use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use rho_providers::model::Message;
use rusqlite::Connection;
use tempfile::TempDir;

use super::super::persistence::{
    session_dir_in_root, session_file_stats, workspace_key, SessionEntry,
};
use super::super::{SessionIndexRecord, SessionSummary};
use super::{
    apply_workspace_updates, indexed_workspace_files, list_workspace_sessions, migrate_index,
    open_index, stale_index_ids, sync_workspace, IndexedFile,
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
    let error = apply_workspace_updates(&mut connection, "ws", &[first, second], &[]).unwrap_err();
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
    apply_workspace_updates(&mut connection, "ws", &[keep, drop], &[]).unwrap();

    let refreshed = test_index_record("keep", "/tmp/keep.jsonl", "kept fresh");
    apply_workspace_updates(&mut connection, "ws", &[refreshed], &["drop".into()]).unwrap();

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
fn stale_index_ids_uses_loaded_map_without_requery() {
    let existing_dir = TempDir::new().unwrap();
    let existing_path = existing_dir.path().join("exists.jsonl");
    fs::write(&existing_path, "").unwrap();
    let mut indexed = HashMap::new();
    indexed.insert(
        "seen-current".into(),
        IndexedFile {
            path: existing_path.to_string_lossy().into_owned(),
            file_size: Some(1),
            file_mtime: Some(1),
            message_count: 1,
            first_user_message: Some("x".into()),
        },
    );
    indexed.insert(
        "seen-path-moved".into(),
        IndexedFile {
            // Missing path must not stale-delete a seen id; upsert repairs it.
            path: "/tmp/definitely-missing-rho-session.jsonl".into(),
            file_size: Some(1),
            file_mtime: Some(1),
            message_count: 0,
            first_user_message: None,
        },
    );
    indexed.insert(
        "seen-unreadable".into(),
        IndexedFile {
            path: "/tmp/definitely-missing-rho-session-unreadable.jsonl".into(),
            file_size: Some(1),
            file_mtime: Some(1),
            message_count: 0,
            first_user_message: None,
        },
    );
    indexed.insert(
        "not-seen".into(),
        IndexedFile {
            path: existing_path.to_string_lossy().into_owned(),
            file_size: Some(1),
            file_mtime: Some(1),
            message_count: 0,
            first_user_message: None,
        },
    );
    let seen = HashSet::from([
        "seen-current".into(),
        "seen-path-moved".into(),
        "seen-unreadable".into(),
    ]);
    let refreshed = HashSet::from(["seen-path-moved"]);
    let mut stale = stale_index_ids(&indexed, &seen, &refreshed);
    stale.sort();
    assert_eq!(
        stale,
        vec!["not-seen".to_string(), "seen-unreadable".to_string()]
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
    let indexed = indexed_workspace_files(&connection, &workspace_key).unwrap();
    let warm = indexed.get("warm").expect("warm row");
    let path = PathBuf::from(&warm.path);
    let (size, mtime) = session_file_stats(&path);
    assert!(
        warm.is_current(&path, size, mtime),
        "freshly synced row must compare current against the loaded map"
    );
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
