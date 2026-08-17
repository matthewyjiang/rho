use rusqlite::{Connection, TransactionBehavior};

use super::PromptHistoryError;

pub(crate) const SCHEMA_VERSION: i64 = 1;

const MIGRATION_1: &str = r#"
CREATE TABLE prompt_entries (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL
);
"#;

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), PromptHistoryError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Read the version after obtaining the write lock. Two processes may both
    // observe a new database before either one starts its migration.
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(PromptHistoryError::UnsupportedSchema {
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
