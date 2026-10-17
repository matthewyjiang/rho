//! The process-wide session index connection and its durability policy.
//!
//! One cached connection per session root, opened through the shared
//! owner-only SQLite lifecycle (WAL, synchronous=NORMAL, busy timeout).

use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

use rusqlite::Connection;

use super::migrate_index;
use crate::sqlite_support::{OwnerOnlySqlite, ParentDirectoryPrivacy};

const INDEX_FILE_NAME: &str = "index.sqlite3";

static INDEX_CONNECTIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>> =
    OnceLock::new();

#[cfg(test)]
pub(in crate::session) fn clear_index_connection_cache_for_test() {
    if let Some(connections) = INDEX_CONNECTIONS.get() {
        connections
            .lock()
            .expect("session index cache poisoned")
            .clear();
    }
}

/// Fsyncs the index WAL so every commit this process can read is durable.
///
/// Index writes use synchronous=NORMAL, which may lose the newest commits on
/// power loss. Writers of data that transcripts cannot rebuild (titles) and
/// readers that persist a position in the `search_changes` journal (whose
/// sequence numbers a rollback would reuse) call this after their commit or
/// read.
/// Committed frames are either in the WAL or were checkpointed into the
/// database file, which SQLite syncs after each completed checkpoint.
///
/// Takes the caller's guard and releases it before the fsync, so saves on
/// other threads are not blocked behind the flush.
pub(in crate::session) fn sync_committed_writes(
    connection: MutexGuard<'_, Connection>,
) -> anyhow::Result<()> {
    let wal = wal_path(&connection);
    drop(connection);
    let Some(wal) = wal else {
        return Ok(());
    };
    // Only the WAL may be opened behind SQLite's back: unix SQLite holds no
    // POSIX locks on it, whereas closing a second handle to the database or
    // `-shm` would drop this process's fcntl locks and risk corruption.
    // Windows flushes only handles opened for writing. Never create the file:
    // without a WAL every commit is already in the synced database file.
    match fs::OpenOptions::new().write(true).open(&wal) {
        Ok(file) => Ok(file.sync_data()?),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// The WAL SQLite actually writes. SQLite names it after the database path it
/// resolved, and unix SQLite resolves a symlinked catalog file, so the
/// requested `index.sqlite3-wal` may not be the live WAL. `None` for a
/// connection without a database file.
fn wal_path(connection: &Connection) -> Option<PathBuf> {
    connection
        .path()
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(format!("{path}-wal")))
}

pub(in crate::session) fn open_index(
    session_root: &Path,
) -> anyhow::Result<Arc<Mutex<Connection>>> {
    let path = session_root.join(INDEX_FILE_NAME);
    let connections = INDEX_CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(connection) = connections
        .lock()
        .expect("session index cache poisoned")
        .get(&path)
    {
        return Ok(Arc::clone(connection));
    }
    // Open without holding the cache lock: opening can wait on another
    // process's write lock, and that wait must not stall other index roots.
    //
    // WAL + synchronous=NORMAL (the shared owner-only lifecycle): every
    // snapshot save upserts this row, and a rollback-journal FULL commit costs
    // four fsyncs. NORMAL may drop the latest commits on power loss: listing
    // reconciles rows against transcript stats, and the rest call
    // `sync_committed_writes`.
    let database = OwnerOnlySqlite::open(
        &path,
        ParentDirectoryPrivacy::EnforcePrivate,
        |connection| migrate_index(connection).map_err(IndexOpenError::from_migration),
    )?;
    let opened = Arc::new(Mutex::new(database.open_write_connection()?));
    // A racing opener may have won; keep its connection so every caller shares one.
    let mut connections = connections.lock().expect("session index cache poisoned");
    Ok(Arc::clone(connections.entry(path).or_insert(opened)))
}

/// Open failures. SQLite errors stay a direct `source()` (not transparent) so
/// `is_lock_contention` sees busy/locked codes and the shared open retries.
#[derive(Debug, thiserror::Error)]
enum IndexOpenError {
    #[error("session index I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("session index SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Migration(anyhow::Error),
}

impl IndexOpenError {
    /// Lifts SQLite failures out of migration's `anyhow` so a busy migration
    /// is retried like any other contended open.
    fn from_migration(error: anyhow::Error) -> Self {
        match error.downcast() {
            Ok(sqlite) => Self::Sqlite(sqlite),
            Err(other) => Self::Migration(other),
        }
    }
}

#[cfg(test)]
#[path = "index_connection_tests.rs"]
mod tests;
