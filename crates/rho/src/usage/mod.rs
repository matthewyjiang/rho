//! Durable, provider-neutral accounting for individual model requests.

mod event;
mod migrations;
mod model_request;
mod recorder;
mod sdk;
mod sqlite;

pub(crate) use model_request::{send_recorded, send_recorded_from_attempt};
pub(crate) use sdk::default_recording;

pub use event::{RequestOutcome, UsageEvent};
pub use recorder::{RecordOutcome, UsageRecorder};
pub use sqlite::SqliteUsageRecorder;

/// Failure to initialize or write the usage ledger.
#[derive(Debug, thiserror::Error)]
pub enum UsageLedgerError {
    #[error("usage ledger I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("usage ledger SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("usage field {field} value {value} exceeds SQLite's signed integer range")]
    IntegerOverflow { field: &'static str, value: u64 },
    #[error("usage ledger schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("could not determine the Rho data directory")]
    DataDirectory,
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
