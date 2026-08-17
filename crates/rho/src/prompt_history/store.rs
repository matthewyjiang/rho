use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use rusqlite::{params, Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior};

use crate::sqlite_privacy::{
    prepare_database_file, prepare_parent_directory, set_sidecar_permissions,
    ParentDirectoryPrivacy,
};

use super::{migrations, PromptHistoryError};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Durable SQLite store for sent composer text. Each operation opens a
/// short-lived connection so independent Rho processes can write concurrently.
///
/// Ops enqueued just before process exit may be dropped when the runtime
/// tears down. The last prompt of a session may occasionally not persist.
#[derive(Clone, Debug)]
pub(crate) struct PromptHistoryStore {
    path: PathBuf,
}

impl PromptHistoryStore {
    /// Opens or creates a history database at `path` and applies migrations.
    #[cfg(test)]
    pub(crate) fn new(path: impl Into<PathBuf>) -> Result<Self, PromptHistoryError> {
        Self::new_with_parent_privacy(path.into(), ParentDirectoryPrivacy::PreserveExisting)
    }

    /// Opens or creates the history database under Rho's configured data root.
    pub(crate) fn at_default_path() -> Result<Self, PromptHistoryError> {
        let path = crate::paths::prompt_history_database_path()
            .map_err(|_| PromptHistoryError::DataDirectory)?;
        Self::new_with_parent_privacy(path, ParentDirectoryPrivacy::EnforcePrivate)
    }

    fn new_with_parent_privacy(
        path: PathBuf,
        parent_privacy: ParentDirectoryPrivacy,
    ) -> Result<Self, PromptHistoryError> {
        prepare_parent_directory(&path, parent_privacy)?;
        prepare_database_file(&path)?;
        let store = Self { path };
        store.initialize()?;
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn initialize(&self) -> Result<(), PromptHistoryError> {
        let deadline = Instant::now() + BUSY_TIMEOUT;
        loop {
            let result = self.open_write_connection().and_then(|mut connection| {
                connection.pragma_update(None, "journal_mode", "WAL")?;
                set_sidecar_permissions(&self.path)?;
                migrations::migrate(&mut connection)?;
                set_sidecar_permissions(&self.path)?;
                Ok(())
            });
            match result {
                Err(error) if is_lock_contention(&error) && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }

    fn open_write_connection(&self) -> Result<Connection, PromptHistoryError> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "cache_size", -512)?;
        Ok(connection)
    }

    /// Inserts `text` unless it matches the newest row, then trims to `max_entries`.
    pub(crate) fn append(
        &self,
        text: &str,
        recorded_at_ms: i64,
        max_entries: usize,
    ) -> Result<(), PromptHistoryError> {
        let mut connection = self.open_write_connection()?;
        set_sidecar_permissions(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last: Option<String> = transaction
            .query_row(
                "SELECT text FROM prompt_entries ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if last.as_deref() != Some(text) {
            transaction.execute(
                "INSERT INTO prompt_entries (recorded_at_ms, text) VALUES (?1, ?2)",
                params![recorded_at_ms, text],
            )?;
            let limit = i64::try_from(max_entries).unwrap_or(i64::MAX);
            transaction.execute(
                "DELETE FROM prompt_entries
                 WHERE id NOT IN (
                     SELECT id FROM prompt_entries ORDER BY id DESC LIMIT ?1
                 )",
                params![limit],
            )?;
        }
        transaction.commit()?;
        set_sidecar_permissions(&self.path)?;
        Ok(())
    }

    /// Returns the newest `limit` entries, oldest first.
    pub(crate) fn load_tail(&self, limit: usize) -> Result<Vec<String>, PromptHistoryError> {
        let connection = self.open_write_connection()?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement =
            connection.prepare("SELECT text FROM prompt_entries ORDER BY id DESC LIMIT ?1")?;
        let mut rows = statement.query(params![limit])?;
        let mut texts = Vec::new();
        while let Some(row) = rows.next()? {
            texts.push(row.get(0)?);
        }
        texts.reverse();
        Ok(texts)
    }

    pub(crate) fn clear(&self) -> Result<(), PromptHistoryError> {
        let mut connection = self.open_write_connection()?;
        set_sidecar_permissions(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM prompt_entries", [])?;
        transaction.commit()?;
        set_sidecar_permissions(&self.path)?;
        Ok(())
    }
}

fn is_lock_contention(error: &PromptHistoryError) -> bool {
    matches!(
        error,
        PromptHistoryError::Sqlite(rusqlite::Error::SqliteFailure(sqlite, _))
            if matches!(sqlite.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}
