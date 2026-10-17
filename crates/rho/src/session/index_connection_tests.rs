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
// writes: beside a symlinked catalog's target, and under a non-UTF-8 root.
// Owner: session index
#[cfg(unix)]
#[test]
fn durability_flush_targets_the_live_wal() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::PathBuf};

    let temp = tempfile::TempDir::new().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let symlinked = base.join("symlinked");
    std::fs::create_dir(&symlinked).unwrap();
    std::fs::write(symlinked.join("catalog.sqlite3"), b"").unwrap();
    std::os::unix::fs::symlink(
        symlinked.join("catalog.sqlite3"),
        symlinked.join("index.sqlite3"),
    )
    .unwrap();
    let non_utf8 = base.join(OsStr::from_bytes(b"caf\xe9"));

    let cases = [
        (symlinked.clone(), symlinked.join("catalog.sqlite3-wal")),
        (non_utf8.clone(), non_utf8.join("index.sqlite3-wal")),
    ];
    let resolved: Vec<(PathBuf, bool)> = cases
        .iter()
        .map(|(root, _)| {
            let connection = open_index(root).unwrap();
            let connection = connection.lock().unwrap();
            connection
                .execute(
                    "insert into sessions (
                        workspace_key, cwd, id, path, created_at, updated_at, message_count
                     ) values ('ws', '/tmp/ws', 'id', '/tmp/ws/id.jsonl', 1, 1, 0)",
                    [],
                )
                .unwrap();
            let wal = wal_path(&connection).unwrap();
            let exists = wal.exists();
            (wal, exists)
        })
        .collect();
    let expected: Vec<(PathBuf, bool)> = cases.into_iter().map(|(_, wal)| (wal, true)).collect();
    assert_eq!(resolved, expected);
}
