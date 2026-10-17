use rusqlite::ErrorCode;

use super::{open_index, wal_path, IndexOpenError};
use crate::sqlite_support::is_lock_contention;

fn sqlite_failure(code: ErrorCode) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code,
            extended_code: 0,
        },
        None,
    )
}

// Covers: a busy migration on first open must reach the shared open's
// contention retry instead of failing the open (and the save it serves).
// Owner: session index
#[test]
fn migration_lock_contention_is_retryable() {
    let errors = [
        anyhow::Error::from(sqlite_failure(ErrorCode::DatabaseBusy)),
        anyhow::Error::from(sqlite_failure(ErrorCode::DatabaseLocked)),
        anyhow::anyhow!("unsupported session index schema 9"),
    ];
    let retryable = errors.map(|error| is_lock_contention(&IndexOpenError::from_migration(error)));
    assert_eq!(retryable, [true, true, false]);
}

// Covers: title and search-cursor durability must flush the WAL SQLite really
// writes; with a symlinked catalog that WAL sits beside the link target.
// Owner: session index
#[cfg(unix)]
#[test]
fn durability_flush_targets_the_wal_of_a_symlinked_catalog() {
    let root = tempfile::TempDir::new().unwrap();
    let target = root.path().join("catalog.sqlite3");
    std::fs::write(&target, b"").unwrap();
    std::os::unix::fs::symlink(&target, root.path().join("index.sqlite3")).unwrap();

    let connection = open_index(root.path()).unwrap();
    let connection = connection.lock().unwrap();
    connection
        .execute(
            "insert into sessions (
                workspace_key, cwd, id, path, created_at, updated_at, message_count
             ) values ('ws', '/tmp/ws', 'id', '/tmp/ws/id.jsonl', 1, 1, 0)",
            [],
        )
        .unwrap();

    let live_wal =
        std::path::PathBuf::from(format!("{}-wal", target.canonicalize().unwrap().display()));
    assert_eq!(
        (wal_path(&connection), live_wal.exists()),
        (Some(live_wal), true)
    );
}
