use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, ErrorCode, OpenFlags, TransactionBehavior};

use super::{create_private_directory, create_private_file, normalize_id, secure_directory};

const INDEX_FILE_NAME: &str = "index.sqlite3";
const INDEX_SCHEMA_VERSION: i64 = 1;
const MAX_ALLOCATION_ATTEMPTS: usize = 100;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub(crate) enum RunPlacement {
    Global {
        parent_session_id: Option<String>,
    },
    Session {
        parent_session_id: String,
        subagents_dir: PathBuf,
    },
}

impl RunPlacement {
    fn parent_session_id(&self) -> Option<&str> {
        match self {
            Self::Global { parent_session_id } => parent_session_id.as_deref(),
            Self::Session {
                parent_session_id, ..
            } => Some(parent_session_id),
        }
    }

    fn directory(&self, global_root: &Path, id: &str) -> PathBuf {
        match self {
            Self::Global { .. } => global_root.join(id),
            Self::Session { subagents_dir, .. } => subagents_dir.join(id),
        }
    }
}

pub(crate) fn reserve_run_directory(placement: &RunPlacement) -> anyhow::Result<(String, PathBuf)> {
    let rho_root = crate::paths::rho_dir()?;
    reserve_run_directory_in_root(&rho_root, placement, new_run_id)
}

fn reserve_run_directory_in_root(
    rho_root: &Path,
    placement: &RunPlacement,
    mut next_id: impl FnMut() -> String,
) -> anyhow::Result<(String, PathBuf)> {
    let global_root = rho_root.join("subagents");
    prepare_private_directory(&global_root)?;
    let index_path = global_root.join(INDEX_FILE_NAME);
    let mut connection = initialize_index(&index_path)?;

    for _ in 0..MAX_ALLOCATION_ATTEMPTS {
        let id = normalize_id(&next_id())?;
        let directory = placement.directory(&global_root, &id);
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let RunPlacement::Session { subagents_dir, .. } = placement {
            prepare_session_subagents_dir(rho_root, subagents_dir)?;
        }
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO runs (run_id, path, parent_session_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                directory.to_string_lossy(),
                placement.parent_session_id(),
                unix_timestamp_secs(),
            ],
        )?;
        if inserted == 0 {
            continue;
        }
        if global_root.join(&id).exists() || !scan_session_directories(rho_root, &id)?.is_empty() {
            continue;
        }

        match create_private_directory(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
        if let Err(error) = transaction.commit() {
            let _ = fs::remove_dir(&directory);
            return Err(error.into());
        }
        set_index_permissions(&index_path)?;
        return Ok((id, directory));
    }

    anyhow::bail!("could not allocate a unique delegated run ID")
}

pub(crate) fn release_run_directory(id: &str, directory: &Path) -> anyhow::Result<()> {
    release_run_directory_in_root(&crate::paths::rho_dir()?, id, directory)
}

fn release_run_directory_in_root(
    rho_root: &Path,
    id: &str,
    directory: &Path,
) -> anyhow::Result<()> {
    let id = normalize_id(id)?;
    validate_indexed_path(rho_root, &id, directory)?;
    let global_root = rho_root.join("subagents");
    let index_path = global_root.join(INDEX_FILE_NAME);
    let mut connection = initialize_index(&index_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match fs::remove_dir_all(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    transaction.execute(
        "DELETE FROM runs WHERE run_id = ?1 AND path = ?2",
        params![id, directory.to_string_lossy()],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn with_parent_run_cleanup_lock_in_root<T>(
    subagents_root: &Path,
    parent_session_id: &str,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    prepare_private_directory(subagents_root)?;
    let index_path = subagents_root.join(INDEX_FILE_NAME);
    let mut connection = initialize_index(&index_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = operation()?;
    transaction.execute(
        "DELETE FROM runs WHERE parent_session_id = ?1",
        params![parent_session_id],
    )?;
    transaction.commit()?;
    Ok(result)
}

pub(crate) fn resolve_run_directory(id: &str) -> anyhow::Result<PathBuf> {
    let rho_root = crate::paths::rho_dir()?;
    resolve_run_directory_in_root(&rho_root, id)
}

fn resolve_run_directory_in_root(rho_root: &Path, id: &str) -> anyhow::Result<PathBuf> {
    let id = normalize_id(id)?;
    let global_root = rho_root.join("subagents");
    let index_path = global_root.join(INDEX_FILE_NAME);

    if index_path.is_file() {
        let connection = initialize_index(&index_path)?;
        let indexed = connection
            .query_row(
                "SELECT path FROM runs WHERE run_id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(path) = indexed {
            let path = PathBuf::from(path);
            validate_indexed_path(rho_root, &id, &path)?;
            if path.is_dir() {
                secure_directory(&path)?;
                return Ok(path);
            }
            connection.execute("DELETE FROM runs WHERE run_id = ?1", params![id])?;
        }
    }

    let matches = scan_session_directories(rho_root, &id)?;
    match matches.as_slice() {
        [directory] => {
            secure_directory(directory)?;
            return Ok(directory.clone());
        }
        [] => {}
        _ => {
            let paths = matches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("delegated run '{id}' is ambiguous across session folders: {paths}");
        }
    }

    let legacy = global_root.join(&id);
    if is_trusted_directory(&global_root) && is_trusted_directory(&legacy) {
        secure_directory(&legacy)?;
        return Ok(legacy);
    }
    anyhow::bail!("unknown delegated run '{id}'")
}

fn prepare_session_subagents_dir(rho_root: &Path, subagents_dir: &Path) -> anyhow::Result<()> {
    let sessions_root = rho_root.join("sessions");
    let relative = subagents_dir.strip_prefix(&sessions_root).map_err(|_| {
        anyhow::anyhow!(
            "session delegated run directory is outside {}",
            sessions_root.display()
        )
    })?;
    let components = relative.components().collect::<Vec<_>>();
    anyhow::ensure!(
        components.len() == 3 && components[2].as_os_str() == "subagents",
        "invalid session delegated run directory: {}",
        subagents_dir.display()
    );

    let workspace = sessions_root.join(components[0].as_os_str());
    let session = workspace.join(components[1].as_os_str());
    for ancestor in [&sessions_root, &workspace, &session] {
        anyhow::ensure!(
            is_trusted_directory(ancestor),
            "{} is not a trusted session directory",
            ancestor.display()
        );
    }
    if subagents_dir.exists() {
        secure_directory(subagents_dir)?;
    } else {
        create_private_directory(subagents_dir)?;
    }
    Ok(())
}

fn scan_session_directories(rho_root: &Path, id: &str) -> anyhow::Result<Vec<PathBuf>> {
    let sessions_root = rho_root.join("sessions");
    let mut matches = Vec::new();
    if !is_trusted_directory(&sessions_root) {
        return Ok(matches);
    }
    for workspace in fs::read_dir(sessions_root)? {
        let workspace = workspace?.path();
        if !is_trusted_directory(&workspace) {
            continue;
        }
        for session in fs::read_dir(workspace)? {
            let session = session?.path();
            if !is_trusted_directory(&session) {
                continue;
            }
            let subagents = session.join("subagents");
            let candidate = subagents.join(id);
            if is_trusted_directory(&subagents) && is_trusted_directory(&candidate) {
                matches.push(candidate);
            }
        }
    }
    matches.sort();
    Ok(matches)
}

fn is_trusted_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn validate_indexed_path(rho_root: &Path, id: &str, path: &Path) -> anyhow::Result<()> {
    let global_root = rho_root.join("subagents");
    let global = path.parent() == Some(global_root.as_path()) && is_trusted_directory(&global_root);
    let sessions_root = rho_root.join("sessions");
    let nested = path
        .strip_prefix(&sessions_root)
        .ok()
        .map(|relative| relative.components().collect::<Vec<_>>())
        .is_some_and(|components| {
            if components.len() != 4
                || components[2].as_os_str() != "subagents"
                || components[3].as_os_str() != id
            {
                return false;
            }
            let workspace = sessions_root.join(components[0].as_os_str());
            let session = workspace.join(components[1].as_os_str());
            let subagents = session.join("subagents");
            is_trusted_directory(&sessions_root)
                && is_trusted_directory(&workspace)
                && is_trusted_directory(&session)
                && is_trusted_directory(&subagents)
        });
    anyhow::ensure!(
        (global || nested) && path.file_name().and_then(|name| name.to_str()) == Some(id),
        "delegated run index contains an invalid path for '{id}': {}",
        path.display()
    );
    Ok(())
}

fn initialize_index(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        prepare_private_directory(parent)?;
    }
    prepare_index_file(path)?;
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match initialize_index_once(path) {
            Err(error) if is_lock_contention(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn initialize_index_once(path: &Path) -> anyhow::Result<Connection> {
    let mut connection = open_index(path)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    anyhow::ensure!(
        current <= INDEX_SCHEMA_VERSION,
        "delegated run index schema {current} is newer than supported schema {INDEX_SCHEMA_VERSION}"
    );
    if current < INDEX_SCHEMA_VERSION {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(
            current <= INDEX_SCHEMA_VERSION,
            "delegated run index schema {current} is newer than supported schema {INDEX_SCHEMA_VERSION}"
        );
        if current < INDEX_SCHEMA_VERSION {
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS runs (
                     run_id TEXT PRIMARY KEY NOT NULL,
                     path TEXT NOT NULL UNIQUE,
                     parent_session_id TEXT,
                     created_at INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS runs_parent_session_idx ON runs(parent_session_id);",
            )?;
            transaction.pragma_update(None, "user_version", INDEX_SCHEMA_VERSION)?;
        }
        transaction.commit()?;
    }
    set_index_permissions(path)?;
    Ok(connection)
}

fn is_lock_contention(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(details, _))
                if matches!(details.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
        )
    })
}

fn open_index(path: &Path) -> anyhow::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(connection)
}

fn prepare_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    secure_directory(path)
}

fn prepare_index_file(path: &Path) -> std::io::Result<()> {
    loop {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        format!("{} is not a trusted database file", path.display()),
                    ));
                }
                return set_index_permissions(path);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => match create_private_file(path) {
                Ok(file) => {
                    drop(file);
                    return set_index_permissions(path);
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            },
            Err(error) => return Err(error),
        }
    }
}

fn set_index_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for candidate in [
            path.to_path_buf(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            if candidate.exists() {
                fs::set_permissions(candidate, fs::Permissions::from_mode(0o600))?;
            }
        }
    }
    Ok(())
}

fn new_run_id() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    id[..6].to_string()
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

use rusqlite::OptionalExtension;

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
