use rusqlite::ErrorCode;

use super::IndexOpenError;
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
