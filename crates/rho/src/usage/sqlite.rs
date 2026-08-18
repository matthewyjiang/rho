use rusqlite::params;

use crate::sqlite_support::{OwnerOnlySqlite, ParentDirectoryPrivacy};

use super::{
    migrations::{self, EVENT_SCHEMA_VERSION},
    RecordOutcome, UsageEvent, UsageLedgerError, UsageRecorder,
};

/// Durable SQLite recorder. It opens a short-lived connection for each write,
/// allowing clones and independent Rho processes to write concurrently.
#[derive(Clone, Debug)]
pub struct SqliteUsageRecorder {
    db: OwnerOnlySqlite,
}

impl SqliteUsageRecorder {
    /// Opens or creates a ledger at `path` and applies all migrations.
    #[cfg(test)]
    pub(crate) fn new(path: impl Into<std::path::PathBuf>) -> Result<Self, UsageLedgerError> {
        Self::open_with(path.into(), ParentDirectoryPrivacy::PreserveExisting)
    }

    /// Opens or creates the ledger under Rho's configured data root.
    pub fn at_default_path() -> Result<Self, UsageLedgerError> {
        let path =
            crate::paths::usage_database_path().map_err(|_| UsageLedgerError::DataDirectory)?;
        Self::open_with(path, ParentDirectoryPrivacy::EnforcePrivate)
    }

    fn open_with(
        path: std::path::PathBuf,
        parent_privacy: ParentDirectoryPrivacy,
    ) -> Result<Self, UsageLedgerError> {
        Ok(Self {
            db: OwnerOnlySqlite::open(path, parent_privacy, migrations::migrate)?,
        })
    }

    #[cfg(test)]
    pub fn path(&self) -> &std::path::Path {
        self.db.path()
    }
}

impl UsageRecorder for SqliteUsageRecorder {
    fn record(&self, event: &UsageEvent) -> Result<RecordOutcome, UsageLedgerError> {
        let step_index = sqlite_integer("step_index", event.step_index)?;
        let attempt_index = sqlite_integer("attempt_index", event.attempt_index)?;
        let input_tokens = sqlite_integer("input_tokens", event.usage.input_tokens)?;
        let output_tokens = sqlite_integer("output_tokens", event.usage.output_tokens)?;
        let cache_read_tokens = sqlite_integer("cache_read_tokens", event.usage.cache_read_tokens)?;
        let cache_write_tokens =
            sqlite_integer("cache_write_tokens", event.usage.cache_write_tokens)?;
        let total_tokens = sqlite_integer("total_tokens", event.usage.total_tokens)?;
        let cost_usd_micros = sqlite_integer("cost_usd_micros", event.usage.cost_usd_micros)?;

        self.db.with_immediate_transaction(|transaction| {
            let changed = transaction.execute(
                "INSERT OR IGNORE INTO usage_events (
                    event_id, schema_version, occurred_at_ms, session_id, parent_session_id,
                    run_id, step_index, attempt_index, workspace_path, provider, model,
                    purpose, request_outcome, input_tokens, output_tokens, cache_read_tokens,
                    cache_write_tokens, total_tokens, cost_usd_micros, rho_version
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
                 )",
                params![
                    event.event_id,
                    EVENT_SCHEMA_VERSION,
                    event.occurred_at_ms,
                    event.session_id,
                    event.parent_session_id,
                    event.run_id,
                    step_index,
                    attempt_index,
                    event.workspace_path,
                    event.provider,
                    event.model,
                    event.purpose,
                    event.outcome.as_str(),
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    total_tokens,
                    cost_usd_micros,
                    event.rho_version,
                ],
            )?;
            Ok(if changed == 1 {
                RecordOutcome::Inserted
            } else {
                RecordOutcome::Duplicate
            })
        })
    }
}

fn sqlite_integer(
    field: &'static str,
    value: Option<u64>,
) -> Result<Option<i64>, UsageLedgerError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| UsageLedgerError::IntegerOverflow { field, value })
        })
        .transpose()
}
