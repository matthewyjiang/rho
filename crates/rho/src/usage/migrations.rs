use rusqlite::{Connection, TransactionBehavior};

use super::UsageLedgerError;

pub(crate) const SCHEMA_VERSION: i64 = 1;
pub(crate) const EVENT_SCHEMA_VERSION: i64 = 1;

const MIGRATION_1: &str = r#"
CREATE TABLE usage_events (
    event_id           TEXT PRIMARY KEY,
    schema_version     INTEGER NOT NULL,
    occurred_at_ms     INTEGER NOT NULL,

    session_id         TEXT,
    parent_session_id  TEXT,
    run_id             TEXT,
    step_index         INTEGER,
    attempt_index      INTEGER,
    workspace_path     TEXT,

    provider           TEXT NOT NULL,
    model              TEXT NOT NULL,
    purpose            TEXT NOT NULL,
    request_outcome    TEXT NOT NULL,

    input_tokens       INTEGER,
    output_tokens      INTEGER,
    cache_read_tokens  INTEGER,
    cache_write_tokens INTEGER,
    total_tokens       INTEGER,
    cost_usd_micros    INTEGER,

    rho_version        TEXT
);

CREATE INDEX usage_events_occurred_at
    ON usage_events (occurred_at_ms);
CREATE INDEX usage_events_session
    ON usage_events (session_id, occurred_at_ms);
"#;

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), UsageLedgerError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Read the version after obtaining the write lock. Two processes may both
    // observe a new database before either one starts its migration.
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(UsageLedgerError::UnsupportedSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }

    if version < 1 {
        transaction.execute_batch(MIGRATION_1)?;
        transaction.pragma_update(None, "user_version", 1)?;
    }
    transaction.commit()?;
    Ok(())
}
