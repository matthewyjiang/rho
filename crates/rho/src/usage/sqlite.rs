use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use rusqlite::{params, Connection, ErrorCode, OpenFlags};

use super::{
    migrations::{self, EVENT_SCHEMA_VERSION},
    RecordOutcome, UsageEvent, UsageLedgerError, UsageRecorder,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Durable SQLite recorder. It opens a short-lived connection for each write,
/// allowing clones and independent Rho processes to write concurrently.
#[derive(Clone, Debug)]
pub struct SqliteUsageRecorder {
    path: PathBuf,
}

impl SqliteUsageRecorder {
    /// Opens or creates a ledger at `path` and applies all migrations.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, UsageLedgerError> {
        let recorder = Self { path: path.into() };
        recorder.initialize()?;
        set_private_file_permissions(&recorder.path)?;
        set_sidecar_permissions(&recorder.path)?;
        Ok(recorder)
    }

    /// Opens or creates the ledger under Rho's configured data root.
    pub fn at_default_path() -> Result<Self, UsageLedgerError> {
        let path =
            crate::paths::usage_database_path().map_err(|_| UsageLedgerError::DataDirectory)?;
        Self::new(path)
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn initialize(&self) -> Result<(), UsageLedgerError> {
        let deadline = Instant::now() + BUSY_TIMEOUT;
        loop {
            let result = self.open_write_connection().and_then(|mut connection| {
                connection.pragma_update(None, "journal_mode", "WAL")?;
                migrations::migrate(&mut connection)
            });
            match result {
                Err(error) if is_lock_contention(&error) && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }

    fn open_write_connection(&self) -> Result<Connection, UsageLedgerError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            set_private_directory_permissions(parent)?;
        }
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(connection)
    }
}

impl UsageRecorder for SqliteUsageRecorder {
    fn record(&self, event: &UsageEvent) -> Result<RecordOutcome, UsageLedgerError> {
        let mut connection = self.open_write_connection()?;
        migrations::migrate(&mut connection)?;

        let step_index = sqlite_integer("step_index", event.step_index)?;
        let attempt_index = sqlite_integer("attempt_index", event.attempt_index)?;
        let input_tokens = sqlite_integer("input_tokens", event.usage.input_tokens)?;
        let output_tokens = sqlite_integer("output_tokens", event.usage.output_tokens)?;
        let cache_read_tokens = sqlite_integer("cache_read_tokens", event.usage.cache_read_tokens)?;
        let cache_write_tokens =
            sqlite_integer("cache_write_tokens", event.usage.cache_write_tokens)?;
        let total_tokens = sqlite_integer("total_tokens", event.usage.total_tokens)?;
        let cost_usd_micros = sqlite_integer("cost_usd_micros", event.usage.cost_usd_micros)?;

        let changed = connection.execute(
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
        set_sidecar_permissions(&self.path)?;
        Ok(if changed == 1 {
            RecordOutcome::Inserted
        } else {
            RecordOutcome::Duplicate
        })
    }
}

fn is_lock_contention(error: &UsageLedgerError) -> bool {
    matches!(
        error,
        UsageLedgerError::Sqlite(rusqlite::Error::SqliteFailure(sqlite, _))
            if matches!(sqlite.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
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

fn set_private_directory_permissions(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn set_sidecar_permissions(path: &Path) -> Result<(), std::io::Error> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.exists() {
            set_private_file_permissions(&sidecar)?;
        }
    }
    Ok(())
}
