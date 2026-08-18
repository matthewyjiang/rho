//! SQLite run index and parent cleanup leases.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};

use crate::sqlite_support::{is_lock_contention, BUSY_TIMEOUT};

use super::super::create_private_file;
use super::paths::prepare_private_directory;

pub(super) const INDEX_FILE_NAME: &str = "index.sqlite3";
const INDEX_SCHEMA_VERSION: i64 = 4;

/// How long a crashed cleanup may block the same parent before the lease is
/// stealable. Fresh leases always block concurrent delete/reserve for that parent.
pub(super) const PARENT_LOCK_TTL_SECS: i64 = 5 * 60;

pub(super) fn initialize_index(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        prepare_private_directory(parent)?;
    }
    prepare_index_file(path)?;
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match initialize_index_once(path) {
            Err(error) if is_lock_contention(error.as_ref()) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn initialize_index_once(path: &Path) -> anyhow::Result<Connection> {
    let mut connection = open_index(path)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    // sqlite's default page cache is 2 MB per connection (cache_size -2000).
    // The index lives for the whole process and touches a handful of pages per
    // run, so cap the cache at 512 KB.
    connection.pragma_update(None, "cache_size", -512)?;
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    anyhow::ensure!(
        current <= INDEX_SCHEMA_VERSION,
        "delegated run index schema {current} is newer than supported schema {INDEX_SCHEMA_VERSION}"
    );
    if current < INDEX_SCHEMA_VERSION {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut current: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(
            current <= INDEX_SCHEMA_VERSION,
            "delegated run index schema {current} is newer than supported schema {INDEX_SCHEMA_VERSION}"
        );
        if current < 1 {
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS runs (
                     run_id TEXT PRIMARY KEY NOT NULL,
                     path TEXT NOT NULL UNIQUE,
                     parent_session_id TEXT,
                     created_at INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS runs_parent_session_idx ON runs(parent_session_id);",
            )?;
            transaction.pragma_update(None, "user_version", 1)?;
            current = 1;
        }
        if current < 2 {
            // Reserved: early PR drafts used user_version=2 without locked_at.
            transaction.pragma_update(None, "user_version", 2)?;
            current = 2;
        }
        if current < 3 {
            // Lease table with stealable locked_at. Rebuild drops any intermediate
            // lock rows from unfinished cleanups.
            transaction.execute_batch(
                "DROP TABLE IF EXISTS parent_locks;
                 CREATE TABLE parent_locks (
                     parent_session_id TEXT PRIMARY KEY NOT NULL,
                     locked_at INTEGER NOT NULL
                 );",
            )?;
            transaction.pragma_update(None, "user_version", 3)?;
            current = 3;
        }
        if current < 4 {
            // Workspace identity for attach-picker scoping. Existing rows stay
            // NULL and stay out of the directory picker; attach by id still finds them.
            transaction.execute_batch(
                "ALTER TABLE runs ADD COLUMN workspace_key TEXT;
                 CREATE INDEX IF NOT EXISTS runs_workspace_idx ON runs(workspace_key);",
            )?;
            transaction.pragma_update(None, "user_version", 4)?;
        }
        transaction.commit()?;
    }
    set_index_permissions(path)?;
    Ok(connection)
}

fn open_index(path: &Path) -> anyhow::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(connection)
}

fn prepare_index_file(path: &Path) -> std::io::Result<()> {
    loop {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        format!("{} is not a trusted database file", path.display()),
                    ));
                }
                return set_index_permissions(path);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => match create_private_file(path) {
                Ok(file) => {
                    drop(file);
                    return set_index_permissions(path);
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            },
            Err(error) => return Err(error),
        }
    }
}

pub(super) fn set_index_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for candidate in [
            path.to_path_buf(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            if candidate.exists() {
                std::fs::set_permissions(candidate, std::fs::Permissions::from_mode(0o600))?;
            }
        }
    }
    let _ = path;
    Ok(())
}

pub(super) fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

fn lock_is_fresh(locked_at: i64, now: i64) -> bool {
    now.saturating_sub(locked_at) < PARENT_LOCK_TTL_SECS
}

/// Acquire or steal a parent cleanup lease.
pub(super) fn acquire_parent_lock(
    transaction: &Transaction<'_>,
    parent_session_id: &str,
    now: i64,
) -> anyhow::Result<()> {
    let existing = transaction
        .query_row(
            "SELECT locked_at FROM parent_locks WHERE parent_session_id = ?1",
            params![parent_session_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    match existing {
        Some(locked_at) if lock_is_fresh(locked_at, now) => {
            anyhow::bail!("parent session '{parent_session_id}' is already being deleted");
        }
        Some(_) => {
            transaction.execute(
                "UPDATE parent_locks SET locked_at = ?1 WHERE parent_session_id = ?2",
                params![now, parent_session_id],
            )?;
        }
        None => {
            transaction.execute(
                "INSERT INTO parent_locks (parent_session_id, locked_at) VALUES (?1, ?2)",
                params![parent_session_id, now],
            )?;
        }
    }
    Ok(())
}

/// Fail when a fresh cleanup lease exists. Stale leases are cleared.
pub(super) fn ensure_parent_not_locked(
    transaction: &Transaction<'_>,
    parent_session_id: &str,
    now: i64,
) -> anyhow::Result<()> {
    let existing = transaction
        .query_row(
            "SELECT locked_at FROM parent_locks WHERE parent_session_id = ?1",
            params![parent_session_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    match existing {
        Some(locked_at) if lock_is_fresh(locked_at, now) => {
            anyhow::bail!("parent session '{parent_session_id}' is being deleted");
        }
        Some(_) => {
            transaction.execute(
                "DELETE FROM parent_locks WHERE parent_session_id = ?1",
                params![parent_session_id],
            )?;
            Ok(())
        }
        None => Ok(()),
    }
}

pub(super) fn clear_parent_index_rows(
    subagents_root: &Path,
    parent_session_id: &str,
) -> anyhow::Result<()> {
    let index_path = subagents_root.join(INDEX_FILE_NAME);
    let mut connection = initialize_index(&index_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM runs WHERE parent_session_id = ?1",
        params![parent_session_id],
    )?;
    transaction.execute(
        "DELETE FROM parent_locks WHERE parent_session_id = ?1",
        params![parent_session_id],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn unlock_parent(subagents_root: &Path, parent_session_id: &str) -> anyhow::Result<()> {
    let index_path = subagents_root.join(INDEX_FILE_NAME);
    if !index_path.is_file() {
        return Ok(());
    }
    let mut connection = initialize_index(&index_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM parent_locks WHERE parent_session_id = ?1",
        params![parent_session_id],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
pub(super) fn insert_parent_lock_for_test(
    subagents_root: &Path,
    parent_session_id: &str,
    locked_at: i64,
) -> anyhow::Result<()> {
    prepare_private_directory(subagents_root)?;
    let index_path = subagents_root.join(INDEX_FILE_NAME);
    let mut connection = initialize_index(&index_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT OR REPLACE INTO parent_locks (parent_session_id, locked_at) VALUES (?1, ?2)",
        params![parent_session_id, locked_at],
    )?;
    transaction.commit()?;
    Ok(())
}
