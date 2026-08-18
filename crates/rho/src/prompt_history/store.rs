use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};

use crate::sqlite_support::{OwnerOnlySqlite, ParentDirectoryPrivacy};

use super::{migrations, PromptHistoryError};

/// Durable SQLite store for sent composer text. Each operation opens a
/// short-lived connection so independent Rho processes can write concurrently.
#[derive(Clone, Debug)]
pub(crate) struct PromptHistoryStore {
    db: OwnerOnlySqlite,
}

impl PromptHistoryStore {
    /// Opens or creates a history database at `path` and applies migrations.
    pub(crate) fn open_path(path: impl Into<PathBuf>) -> Result<Self, PromptHistoryError> {
        Self::open_with(path.into(), ParentDirectoryPrivacy::PreserveExisting)
    }

    /// Opens an existing history database at `path`, or `None` if the file is absent.
    pub(crate) fn open_path_if_exists(
        path: impl Into<PathBuf>,
    ) -> Result<Option<Self>, PromptHistoryError> {
        Self::open_existing_with(path.into(), ParentDirectoryPrivacy::PreserveExisting)
    }

    /// Opens or creates the history database under Rho's configured data root.
    pub(crate) fn at_default_path() -> Result<Self, PromptHistoryError> {
        Self::open_with(default_path()?, ParentDirectoryPrivacy::EnforcePrivate)
    }

    /// Opens the default history database when the file already exists.
    pub(crate) fn at_default_path_if_exists() -> Result<Option<Self>, PromptHistoryError> {
        Self::open_existing_with(default_path()?, ParentDirectoryPrivacy::EnforcePrivate)
    }

    fn open_with(
        path: PathBuf,
        parent_privacy: ParentDirectoryPrivacy,
    ) -> Result<Self, PromptHistoryError> {
        Ok(Self {
            db: OwnerOnlySqlite::open(path, parent_privacy, migrations::migrate)?,
        })
    }

    fn open_existing_with(
        path: PathBuf,
        parent_privacy: ParentDirectoryPrivacy,
    ) -> Result<Option<Self>, PromptHistoryError> {
        Ok(
            OwnerOnlySqlite::open_existing(path, parent_privacy, migrations::migrate)?
                .map(|db| Self { db }),
        )
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &std::path::Path {
        self.db.path()
    }

    /// Inserts `text` unless it matches the newest row, then trims to `max_entries`.
    pub(crate) fn append(&self, text: &str, max_entries: usize) -> Result<(), PromptHistoryError> {
        self.db.with_immediate_transaction(|transaction| {
            let last: Option<String> = transaction
                .query_row(
                    "SELECT text FROM prompt_entries ORDER BY id DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if last.as_deref() != Some(text) {
                transaction.execute(
                    "INSERT INTO prompt_entries (text) VALUES (?1)",
                    params![text],
                )?;
                trim_to_newest(transaction, max_entries)?;
            }
            Ok(())
        })
    }

    pub(crate) fn count(&self) -> Result<usize, PromptHistoryError> {
        let connection = self.db.open_write_connection()?;
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM prompt_entries", [], |row| row.get(0))?;
        usize::try_from(count).map_err(|_| {
            PromptHistoryError::Sqlite(rusqlite::Error::IntegralValueOutOfRange(0, count))
        })
    }

    /// Drops oldest rows until at most `max_entries` remain.
    pub(crate) fn enforce_limit(&self, max_entries: usize) -> Result<(), PromptHistoryError> {
        self.db
            .with_immediate_transaction(|transaction| trim_to_newest(transaction, max_entries))
    }

    /// Returns the newest `limit` entries, oldest first.
    pub(crate) fn load_tail(&self, limit: usize) -> Result<Vec<String>, PromptHistoryError> {
        let connection = self.db.open_write_connection()?;
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
        self.db.with_immediate_transaction(|transaction| {
            transaction.execute("DELETE FROM prompt_entries", [])?;
            Ok(())
        })
    }
}

fn default_path() -> Result<PathBuf, PromptHistoryError> {
    crate::paths::prompt_history_database_path().map_err(|_| PromptHistoryError::DataDirectory)
}

fn trim_to_newest(
    transaction: &rusqlite::Transaction<'_>,
    max_entries: usize,
) -> Result<(), PromptHistoryError> {
    let limit = i64::try_from(max_entries).unwrap_or(i64::MAX);
    transaction.execute(
        "DELETE FROM prompt_entries
         WHERE id NOT IN (
             SELECT id FROM prompt_entries ORDER BY id DESC LIMIT ?1
         )",
        params![limit],
    )?;
    Ok(())
}
