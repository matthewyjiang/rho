//! Owner-only SQLite files and the shared open / migrate / lock lifecycle.

use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction, TransactionBehavior};

pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) enum ParentDirectoryPrivacy {
    PreserveExisting,
    EnforcePrivate,
}

/// Short-lived connection factory for a private on-disk SQLite file.
#[derive(Clone, Debug)]
pub(crate) struct OwnerOnlySqlite {
    path: PathBuf,
}

impl OwnerOnlySqlite {
    pub(crate) fn open<E>(
        path: impl Into<PathBuf>,
        parent_privacy: ParentDirectoryPrivacy,
        migrate: impl FnMut(&mut Connection) -> Result<(), E>,
    ) -> Result<Self, E>
    where
        E: From<std::io::Error> + From<rusqlite::Error> + std::error::Error + 'static,
    {
        let path = path.into();
        prepare_parent_directory(&path, parent_privacy)?;
        prepare_database_file(&path)?;
        let database = Self { path };
        database.initialize(migrate)?;
        Ok(database)
    }

    pub(crate) fn open_existing<E>(
        path: impl Into<PathBuf>,
        parent_privacy: ParentDirectoryPrivacy,
        migrate: impl FnMut(&mut Connection) -> Result<(), E>,
    ) -> Result<Option<Self>, E>
    where
        E: From<std::io::Error> + From<rusqlite::Error> + std::error::Error + 'static,
    {
        let path = path.into();
        if !path.is_file() {
            return Ok(None);
        }
        Self::open(path, parent_privacy, migrate).map(Some)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn open_write_connection(&self) -> Result<Connection, rusqlite::Error> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        // sqlite's default page cache is 2 MB per connection (cache_size
        // -2000). These stores live for the whole process and touch a
        // handful of pages per write, so cap the cache at 512 KB.
        connection.pragma_update(None, "cache_size", -512)?;
        Ok(connection)
    }

    pub(crate) fn with_immediate_transaction<T, E>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<std::io::Error> + From<rusqlite::Error>,
    {
        let mut connection = self.open_write_connection()?;
        set_sidecar_permissions(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&transaction)?;
        transaction.commit()?;
        set_sidecar_permissions(&self.path)?;
        Ok(value)
    }

    fn initialize<E>(
        &self,
        mut migrate: impl FnMut(&mut Connection) -> Result<(), E>,
    ) -> Result<(), E>
    where
        E: From<std::io::Error> + From<rusqlite::Error> + std::error::Error + 'static,
    {
        let deadline = Instant::now() + BUSY_TIMEOUT;
        loop {
            let result =
                self.open_write_connection()
                    .map_err(E::from)
                    .and_then(|mut connection| {
                        connection.pragma_update(None, "journal_mode", "WAL")?;
                        set_sidecar_permissions(&self.path)?;
                        migrate(&mut connection)?;
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
}

pub(crate) fn is_lock_contention(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(err) = current {
        if let Some(rusqlite::Error::SqliteFailure(details, _)) =
            err.downcast_ref::<rusqlite::Error>()
        {
            if matches!(
                details.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            ) {
                return true;
            }
        }
        current = err.source();
    }
    false
}

fn prepare_parent_directory(
    path: &Path,
    privacy: ParentDirectoryPrivacy,
) -> Result<(), std::io::Error> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;

    if matches!(privacy, ParentDirectoryPrivacy::EnforcePrivate) {
        set_private_directory_permissions(parent)?;
    }
    Ok(())
}

fn prepare_database_file(path: &Path) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    set_private_file_permissions(path)
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
        match set_private_file_permissions(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::ErrorCode;

    fn sqlite_failure(code: ErrorCode) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code,
                extended_code: 0,
            },
            None,
        )
    }

    // Covers: busy and locked codes are contention; other sqlite errors are not.
    // Owner: sqlite support
    #[test]
    fn lock_contention_detects_busy_and_locked() {
        assert!(super::is_lock_contention(&sqlite_failure(
            ErrorCode::DatabaseBusy
        )));
        assert!(super::is_lock_contention(&sqlite_failure(
            ErrorCode::DatabaseLocked
        )));
        assert!(!super::is_lock_contention(&sqlite_failure(
            ErrorCode::ConstraintViolation
        )));
    }
}
