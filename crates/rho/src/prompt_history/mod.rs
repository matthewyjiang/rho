//! Shared, durable composer prompt history for the interactive TUI.

mod migrations;
mod store;

pub(crate) use store::PromptHistoryStore;

/// Tail load result: store handle, oldest-first entries, and the configured cap.
pub(crate) type PromptHistorySnapshot = (PromptHistoryStore, Vec<String>, usize);
pub(crate) type PromptHistoryLoadHandle = tokio::task::JoinHandle<Option<PromptHistorySnapshot>>;

/// Failure to initialize or write the shared prompt history database.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PromptHistoryError {
    #[error("prompt history I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("prompt history SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("prompt history schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("could not determine the Rho data directory")]
    DataDirectory,
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
