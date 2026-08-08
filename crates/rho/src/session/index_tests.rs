use std::{fs, path::PathBuf};

use rusqlite::{params, Connection};
use tempfile::TempDir;

use super::{
    matching_sessions_any_workspace, migrate_index, migrate_index_with_hook, open_index,
    validate_index_columns, INDEX_SCHEMA_VERSION,
};

const INDEX_V0: &str = include_str!("fixtures/index-v0.sql");
const INDEX_V1: &str = include_str!("fixtures/index-v1.sql");

#[test]
fn every_supported_index_fixture_migrates_transactionally() {
    for (source_version, fixture) in [(0, INDEX_V0), (1, INDEX_V1)] {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(fixture).unwrap();
        let before: u32 = connection
            .query_row("pragma user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, source_version);

        migrate_index(&mut connection).unwrap();

        let after: u32 = connection
            .query_row("pragma user_version", [], |row| row.get(0))
            .unwrap();
        let title: Option<String> = connection
            .query_row(
                "select title from sessions where id = 'fixture-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, INDEX_SCHEMA_VERSION);
        if source_version == 1 {
            assert_eq!(title.as_deref(), Some("fixture title"));
            let file_size: Option<i64> = connection
                .query_row(
                    "select file_size from sessions where id = 'fixture-session'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(file_size, None);
        }
        validate_index_columns(&connection).unwrap();
    }
}

#[test]
fn rejects_newer_and_malformed_index_schemas() {
    let mut newer = Connection::open_in_memory().unwrap();
    newer
        .pragma_update(None, "user_version", INDEX_SCHEMA_VERSION + 1)
        .unwrap();
    let error = migrate_index(&mut newer).unwrap_err();
    assert!(error
        .to_string()
        .contains("unsupported session index schema"));

    let mut malformed = Connection::open_in_memory().unwrap();
    malformed
        .execute_batch(
            "pragma user_version = 1;
             create table sessions (workspace_key text not null);",
        )
        .unwrap();
    let error = migrate_index(&mut malformed).unwrap_err();
    assert!(error.to_string().contains("malformed session index schema"));
}

#[test]
fn failed_index_migration_rolls_back_every_schema_change() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(INDEX_V0).unwrap();

    let error = migrate_index_with_hook(&mut connection, |_| {
        anyhow::bail!("injected migration failure")
    })
    .unwrap_err();

    assert!(error.to_string().contains("injected migration failure"));
    let version: u32 = connection
        .query_row("pragma user_version", [], |row| row.get(0))
        .unwrap();
    let mut statement = connection.prepare("pragma table_info(sessions)").unwrap();
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(version, 0);
    assert!(!columns.iter().any(|column| column == "title"));
    assert!(!columns.iter().any(|column| column == "first_user_message"));
}

#[test]
fn open_index_creates_schema() {
    let root = TempDir::new().unwrap();
    let connection = open_index(root.path()).unwrap();
    let connection = connection.lock().unwrap();

    let table_count: i64 = connection
        .query_row(
            "select count(*) from sqlite_master where type = 'table' and name = 'sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(table_count, 1);
    let version: u32 = connection
        .query_row("pragma user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, INDEX_SCHEMA_VERSION);
    assert!(root.path().join("index.sqlite3").exists());
}

#[test]
fn matching_sessions_any_workspace_dedupes_same_path_across_cwds() {
    let root = TempDir::new().unwrap();
    let session_path = root.path().join("session.jsonl");
    fs::write(&session_path, "").unwrap();
    let path = session_path.to_string_lossy().to_string();
    let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    {
        let connection = open_index(root.path()).unwrap();
        let connection = connection.lock().unwrap();
        // Same session file indexed under two workspace keys with different cwds.
        for (workspace_key, cwd, updated_at) in [
            ("ws-stale", "/tmp/old-workspace", 10_i64),
            ("ws-fresh", "/tmp/new-workspace", 20_i64),
        ] {
            connection
                .execute(
                    "insert into sessions (
                        workspace_key, cwd, id, path, created_at, updated_at, message_count
                     ) values (?1, ?2, ?3, ?4, 1, ?5, 0)",
                    params![workspace_key, cwd, id, path.as_str(), updated_at],
                )
                .unwrap();
        }
    }

    let matches = matching_sessions_any_workspace(root.path(), "aaaaaaaa").unwrap();
    assert_eq!(
        matches,
        vec![(session_path, PathBuf::from("/tmp/new-workspace"))],
        "one path with differing indexed cwds must resolve as a single match"
    );
}
