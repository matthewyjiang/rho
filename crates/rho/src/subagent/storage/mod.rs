//! Nested and global delegated-run artifact placement.

mod index;
mod paths;

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{create_private_directory, normalize_id, secure_directory};
use index::{
    acquire_parent_lock, clear_parent_index_rows, ensure_parent_not_locked, initialize_index,
    set_index_permissions, unix_timestamp_secs, unlock_parent, INDEX_FILE_NAME,
};
use paths::{
    prepare_private_directory, scan_session_directories, validate_run_directory,
    SessionSubagentsDir,
};

pub(crate) use paths::is_trusted_directory;

const MAX_ALLOCATION_ATTEMPTS: usize = 100;

/// One row from the global delegated-run index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedRun {
    pub id: String,
    pub directory: PathBuf,
}

/// A non-terminal indexed run, ready for attach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunningRun {
    pub id: String,
    pub agent_id: String,
    pub title: Option<String>,
    pub last_activity: Option<String>,
    pub state: super::RunState,
    pub elapsed_seconds: u64,
}

fn list_indexed_runs_in_root(
    rho_root: &Path,
    workspace_key: &str,
) -> anyhow::Result<Vec<IndexedRun>> {
    let index_path = rho_root.join("subagents").join(INDEX_FILE_NAME);
    if !index_path.is_file() {
        return Ok(Vec::new());
    }
    let connection = initialize_index(&index_path)?;
    let mut statement = connection.prepare(
        "SELECT run_id, path FROM runs WHERE workspace_key = ?1 ORDER BY created_at DESC",
    )?;
    let rows = statement.query_map(params![workspace_key], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut runs = Vec::new();
    for row in rows {
        let (id, path) = row?;
        let directory = PathBuf::from(path);
        if validate_run_directory(rho_root, &id, &directory).is_ok()
            && is_trusted_directory(&directory)
        {
            runs.push(IndexedRun { id, directory });
        }
    }
    Ok(runs)
}

/// Indexed non-terminal runs started in `cwd`.
///
/// Membership comes from the workspace key written at reservation. Rows with a
/// missing key (pre-migration) stay out of the picker; `rho attach <id>` still
/// finds them from any directory.
pub(crate) fn list_running_runs(cwd: &Path) -> anyhow::Result<Vec<RunningRun>> {
    list_running_runs_in_root(&crate::paths::rho_dir()?, cwd)
}

fn list_running_runs_in_root(rho_root: &Path, cwd: &Path) -> anyhow::Result<Vec<RunningRun>> {
    let now = super::unix_now_secs();
    let Some(workspace_key) = run_workspace_key(cwd) else {
        return Ok(Vec::new());
    };
    let mut running = Vec::new();
    for indexed in list_indexed_runs_in_root(rho_root, &workspace_key)? {
        let Some(status) = super::read_status(&indexed.directory.join(super::RESULT_FILE_NAME))
        else {
            continue;
        };
        if status.state.is_terminal() {
            continue;
        }
        let elapsed_seconds = status
            .elapsed_duration(now)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        running.push(RunningRun {
            id: indexed.id,
            agent_id: status.agent_id.unwrap_or_else(|| "agent".into()),
            title: status.title,
            last_activity: status.last_activity,
            state: status.state,
            elapsed_seconds,
        });
    }
    Ok(running)
}

/// Where a delegated run's artifact directory should live.
#[derive(Clone, Debug)]
pub(crate) enum RunPlacement {
    /// Global pool under `~/.rho/subagents/<id>/`.
    Global { parent_session_id: Option<String> },
    /// Nested under a folder-layout session's `subagents/` directory.
    Session {
        parent_session_id: String,
        subagents_dir: PathBuf,
    },
}

impl RunPlacement {
    /// Parentless automation / unbound manager default.
    pub(crate) fn parentless() -> Self {
        Self::Global {
            parent_session_id: None,
        }
    }

    /// Interactive parent binding: nest when the session owns a folder.
    pub(crate) fn for_parent_session(
        session_id: impl Into<String>,
        subagents_dir: Option<PathBuf>,
    ) -> Self {
        let session_id = session_id.into();
        match subagents_dir {
            Some(subagents_dir) => Self::Session {
                parent_session_id: session_id,
                subagents_dir,
            },
            None => Self::Global {
                parent_session_id: Some(session_id),
            },
        }
    }

    pub(crate) fn parent_session_id(&self) -> Option<&str> {
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

/// Exclusive cleanup claim for one parent session's indexed runs.
///
/// While held, reservations that record this parent fail fast. Dropping without
/// [`Self::clear_index_and_unlock`] only releases the claim so a failed delete
/// does not leave the parent permanently blocked. Crashed holders expire after
/// [`index::PARENT_LOCK_TTL_SECS`] and become stealable.
#[derive(Debug)]
pub(crate) struct ParentRunCleanupGuard {
    subagents_root: PathBuf,
    parent_session_id: String,
    released: bool,
}

impl ParentRunCleanupGuard {
    /// Remove index rows for this parent while retaining the cleanup claim.
    pub(crate) fn clear_index(&self) -> anyhow::Result<()> {
        clear_parent_index_rows(&self.subagents_root, &self.parent_session_id)
    }

    #[cfg(test)]
    /// Remove index rows for this parent and release the cleanup claim.
    pub(crate) fn clear_index_and_unlock(mut self) -> anyhow::Result<()> {
        clear_parent_index_rows(&self.subagents_root, &self.parent_session_id)?;
        self.released = true;
        Ok(())
    }
}

impl Drop for ParentRunCleanupGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let _ = unlock_parent(&self.subagents_root, &self.parent_session_id);
    }
}

/// Workspace key used for attach-picker membership.
///
/// Canonicalize first so a reserved run and a later `rho attach` from the same
/// directory share a key even when one caller passed `Workspace::root()` and
/// the other passed `current_dir()`. Empty paths have no key.
fn run_workspace_key(cwd: &Path) -> Option<String> {
    if cwd.as_os_str().is_empty() {
        return None;
    }
    let path = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    Some(crate::session::workspace_key(&path))
}

pub(crate) fn reserve_run_directory(
    placement: &RunPlacement,
    cwd: &Path,
) -> anyhow::Result<(String, PathBuf)> {
    let rho_root = crate::paths::rho_dir()?;
    reserve_run_directory_in_root(&rho_root, cwd, placement, new_run_id)
}

fn reserve_run_directory_in_root(
    rho_root: &Path,
    cwd: &Path,
    placement: &RunPlacement,
    mut next_id: impl FnMut() -> String,
) -> anyhow::Result<(String, PathBuf)> {
    let global_root = rho_root.join("subagents");
    prepare_private_directory(&global_root)?;
    let index_path = global_root.join(INDEX_FILE_NAME);
    let mut connection = initialize_index(&index_path)?;
    if let RunPlacement::Session { subagents_dir, .. } = placement {
        SessionSubagentsDir::parse(rho_root, subagents_dir)?.ensure_ready()?;
    }

    for _ in 0..MAX_ALLOCATION_ATTEMPTS {
        let id = normalize_id(&next_id())?;
        let directory = placement.directory(&global_root, &id);
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(parent_session_id) = placement.parent_session_id() {
            ensure_parent_not_locked(&transaction, parent_session_id, unix_timestamp_secs())?;
        }
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO runs (run_id, path, parent_session_id, created_at, workspace_key)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                directory.to_string_lossy(),
                placement.parent_session_id(),
                unix_timestamp_secs(),
                run_workspace_key(cwd),
            ],
        )?;
        if inserted == 0 {
            continue;
        }

        // Index PK is the uniqueness authority. Target-path create_new handles
        // same-location orphans; resolve keeps scan only as a fallback.
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
    validate_run_directory(rho_root, &id, directory)?;
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

/// Claim exclusive cleanup for `parent_session_id` so new reservations that
/// record that parent fail until the guard is released.
pub(crate) fn lock_parent_for_cleanup(
    subagents_root: &Path,
    parent_session_id: &str,
) -> anyhow::Result<ParentRunCleanupGuard> {
    lock_parent_for_cleanup_in_root(subagents_root, parent_session_id)
}

fn lock_parent_for_cleanup_in_root(
    subagents_root: &Path,
    parent_session_id: &str,
) -> anyhow::Result<ParentRunCleanupGuard> {
    prepare_private_directory(subagents_root)?;
    let index_path = subagents_root.join(INDEX_FILE_NAME);
    let mut connection = initialize_index(&index_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    acquire_parent_lock(&transaction, parent_session_id, unix_timestamp_secs())?;
    transaction.commit()?;
    Ok(ParentRunCleanupGuard {
        subagents_root: subagents_root.to_path_buf(),
        parent_session_id: parent_session_id.to_string(),
        released: false,
    })
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
            validate_run_directory(rho_root, &id, &path)?;
            if is_trusted_directory(&path) {
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

fn new_run_id() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    id[..6].to_string()
}
