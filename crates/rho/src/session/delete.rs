//! Session delete with cascade cleanup of parent-linked subagent runs.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[cfg(test)]
use crate::subagent::RunStatus;
use crate::subagent::{self, RunIndexCleanup, RunState, RESULT_FILE_NAME};

use super::{
    acquire_delete_session_lease, index,
    persistence::{workspace_key, ResolvedSession, SessionStore, SessionUnit},
    SessionSummary, SessionTarget,
};

/// Controls for [`super::Session::delete_target`] and batch deletion.
#[derive(Clone, Debug, Default)]
pub struct DeleteOptions {
    /// When true, delete even if a parent-linked run is still non-terminal.
    ///
    /// Only intended for stale `Running`/`Starting` artifacts left behind after a
    /// crash. Live runs may still be writing; prefer waiting for completion.
    pub force: bool,
    /// Refuse delete when the resolved session has this exact workspace identity.
    pub protected_session: Option<SessionTarget>,
}

/// Result of a successful session delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteOutcome {
    pub id: String,
    pub cwd: PathBuf,
    pub path: PathBuf,
    /// Number of nested and global parent-linked run directories removed.
    pub deleted_run_count: usize,
    /// Run ids force-deleted while still non-terminal.
    pub forced_run_ids: Vec<String>,
}

/// One session that cleanup could not remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CleanupFailure {
    pub(crate) id: String,
    pub(crate) cwd: PathBuf,
    pub(crate) error: String,
}

/// Result of an explicit cleanup of sessions for missing workspace directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CleanupOutcome {
    pub(crate) deleted: Vec<DeleteOutcome>,
    pub(crate) failures: Vec<CleanupFailure>,
    /// Candidates skipped because their workspace reappeared after preview.
    pub(crate) restored_workspaces: usize,
}

/// Result of deleting every deletable session owned by one workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceDeleteOutcome {
    pub(crate) deleted: Vec<DeleteOutcome>,
    pub(crate) failures: Vec<CleanupFailure>,
    pub(crate) kept_protected: Vec<SessionTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunCleanup {
    Structural,
    Explicit,
}

#[derive(Clone, Debug)]
struct LinkedRun {
    dir: PathBuf,
    id: String,
    state: Option<RunState>,
    cleanup: RunCleanup,
}

pub(super) fn list_missing_workspaces_in_root(
    session_root: &Path,
) -> anyhow::Result<Vec<SessionSummary>> {
    index::list_all_sessions(session_root)?
        .into_iter()
        .filter_map(
            |session| match workspace_directory_is_missing(&session.cwd) {
                Ok(true) => Some(Ok(session)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

#[cfg(test)]
pub(super) fn cleanup_missing_workspaces_in_roots(
    session_root: &Path,
    subagents_root: &Path,
    options: &DeleteOptions,
) -> anyhow::Result<CleanupOutcome> {
    let targets = list_missing_workspaces_in_root(session_root)?
        .into_iter()
        .map(|session| session.target())
        .collect::<Vec<_>>();
    cleanup_missing_targets_in_roots(session_root, subagents_root, &targets, options)
}

pub(super) fn cleanup_missing_targets_in_roots(
    session_root: &Path,
    subagents_root: &Path,
    targets: &[SessionTarget],
    options: &DeleteOptions,
) -> anyhow::Result<CleanupOutcome> {
    let mut batch = BatchDelete::open(session_root, subagents_root, options)?;
    let mut outcome = CleanupOutcome::default();
    for target in targets {
        // Confirmation and deletion are separate operations. Re-check only the
        // reviewed target so restored workspaces are kept and new sessions are
        // never swept into an old confirmation.
        match workspace_directory_is_missing(&target.cwd) {
            Ok(true) => {}
            Ok(false) => {
                outcome.restored_workspaces += 1;
                continue;
            }
            Err(error) => {
                outcome.failures.push(CleanupFailure {
                    id: target.id.clone(),
                    cwd: target.cwd.clone(),
                    error: error.to_string(),
                });
                continue;
            }
        }
        match batch.delete_target(target) {
            Ok(deleted) => outcome.deleted.push(deleted),
            Err(error) => outcome.failures.push(CleanupFailure {
                id: target.id.clone(),
                cwd: target.cwd.clone(),
                error: error.to_string(),
            }),
        }
    }
    batch.finish();
    Ok(outcome)
}

pub(super) fn workspace_directory_is_missing(cwd: &Path) -> anyhow::Result<bool> {
    match fs::metadata(cwd) {
        Ok(metadata) => Ok(!metadata.is_dir()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(anyhow::anyhow!(
            "could not inspect workspace directory {}: {error}",
            cwd.display()
        )),
    }
}

#[cfg(test)]
pub(super) fn delete_in_roots(
    session_root: &Path,
    subagents_root: &Path,
    cwd: &Path,
    id_prefix: &str,
    options: &DeleteOptions,
) -> anyhow::Result<DeleteOutcome> {
    let resolved = SessionStore::new(session_root, cwd).resolve(id_prefix)?;
    let mut batch = BatchDelete::open(session_root, subagents_root, options)?;
    let outcome = batch.delete_resolved(resolved)?;
    batch.finish();
    Ok(outcome)
}

pub(super) fn delete_target_in_roots(
    session_root: &Path,
    subagents_root: &Path,
    target: &SessionTarget,
    options: &DeleteOptions,
) -> anyhow::Result<DeleteOutcome> {
    let mut batch = BatchDelete::open(session_root, subagents_root, options)?;
    let outcome = batch.delete_target(target)?;
    batch.finish();
    Ok(outcome)
}

pub(super) fn delete_targets_in_roots(
    session_root: &Path,
    subagents_root: &Path,
    targets: &[SessionTarget],
    options: &DeleteOptions,
) -> anyhow::Result<WorkspaceDeleteOutcome> {
    let mut batch = BatchDelete::open(session_root, subagents_root, options)?;
    let mut outcome = WorkspaceDeleteOutcome::default();
    for target in targets {
        if options.protected_session.as_ref() == Some(target) {
            outcome.kept_protected.push(target.clone());
            continue;
        }
        match batch.delete_target(target) {
            Ok(deleted) => outcome.deleted.push(deleted),
            Err(error) => outcome.failures.push(CleanupFailure {
                id: target.id.clone(),
                cwd: target.cwd.clone(),
                error: error.to_string(),
            }),
        }
    }
    batch.finish();
    Ok(outcome)
}

/// One delete pass over any number of sessions.
///
/// Index bookkeeping, not file removal, dominated bulk cleanup: each session
/// reopened the WAL run index three times (each close checkpoints and syncs)
/// and made its own session-index commit, about 18ms per session and 10s for
/// 523 sessions. The batch keeps one run-index handle open and drops
/// session-index rows in one transaction in [`Self::finish`]. Deferring those
/// rows is safe: every reader either reconciles first or skips rows whose
/// transcript is gone, and reconciliation drops rows `finish` never reached.
struct BatchDelete<'a> {
    session_root: &'a Path,
    subagents_root: &'a Path,
    runs: RunIndexCleanup,
    options: &'a DeleteOptions,
    /// Index rows of sessions removed from disk.
    removed: Vec<index::IndexKey>,
}

impl<'a> BatchDelete<'a> {
    fn open(
        session_root: &'a Path,
        subagents_root: &'a Path,
        options: &'a DeleteOptions,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            session_root,
            subagents_root,
            runs: RunIndexCleanup::open(subagents_root)?,
            options,
            removed: Vec::new(),
        })
    }

    fn delete_target(&mut self, target: &SessionTarget) -> anyhow::Result<DeleteOutcome> {
        let resolved =
            SessionStore::new(self.session_root, &target.cwd).resolve_in_workspace(&target.id)?;
        self.delete_resolved(resolved)
    }

    fn delete_resolved(&mut self, resolved: ResolvedSession) -> anyhow::Result<DeleteOutcome> {
        let options = self.options;
        if options
            .protected_session
            .as_ref()
            .is_some_and(|protected| protected.id == resolved.id && protected.cwd == resolved.cwd)
        {
            anyhow::bail!(
                "refusing to delete the current session '{}'; start a new session or resume another first",
                short_id(&resolved.id)
            );
        }

        let unit = SessionUnit::from_path(&resolved.path).ok_or_else(|| {
            anyhow::anyhow!(
                "session '{}' has an unrecognized on-disk layout at {}",
                resolved.id,
                resolved.path.display()
            )
        })?;

        let _session_lease =
            acquire_delete_session_lease(self.session_root, &resolved.cwd, &resolved.id)?;
        let cleanup_guard = self.runs.lock_parent(&resolved.id)?;

        let mut linked = find_nested_runs(&unit)?;
        linked.extend(find_parent_linked_runs(self.subagents_root, &resolved.id)?);
        linked.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.dir.cmp(&right.dir))
        });

        let mut forced_run_ids = Vec::new();
        for run in &linked {
            if run.state.is_some_and(RunState::is_terminal) {
                continue;
            }
            if !options.force {
                let crash_hint =
                    if matches!(run.state, Some(RunState::Running | RunState::Starting)) {
                        " (use --force only for stale runs left after a crash)"
                    } else {
                        ""
                    };
                let state = run.state.map(RunState::as_str).unwrap_or("unknown");
                anyhow::bail!(
                    "refusing to delete session '{}': related run {} is still {state}{crash_hint}; wait for it to finish or pass --force",
                    short_id(&resolved.id),
                    run.id,
                );
            }
            forced_run_ids.push(run.id.clone());
        }

        // Finish every fallible side cleanup while the transcript still exists,
        // so any error leaves an exact target that the caller can retry.
        for run in linked
            .iter()
            .filter(|run| run.cleanup == RunCleanup::Explicit)
        {
            match fs::remove_dir_all(&run.dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "could not remove related run {} before deleting session '{}': {error}",
                        run.id,
                        resolved.id
                    ));
                }
            }
        }
        cleanup_guard.clear_index()?;
        unit.delete_from_disk()?;
        drop(cleanup_guard);
        self.removed
            .push((workspace_key(&resolved.cwd), resolved.id.clone()));

        Ok(DeleteOutcome {
            deleted_run_count: linked.len(),
            id: resolved.id,
            cwd: resolved.cwd,
            path: resolved.path,
            forced_run_ids,
        })
    }

    /// Drop index rows for every deleted session in one commit.
    ///
    /// Best effort: the sessions are already gone from disk, and a leftover
    /// row is ignored by readers and dropped by the next reconcile, so an
    /// index failure must not report a finished delete as failed.
    fn finish(self) {
        if let Err(error) = index::remove_sessions(self.session_root, &self.removed) {
            tracing::warn!(%error, "could not drop deleted sessions from the session index");
        }
    }
}

fn find_nested_runs(unit: &SessionUnit) -> anyhow::Result<Vec<LinkedRun>> {
    let Some(subagents_dir) = unit.subagents_dir() else {
        return Ok(Vec::new());
    };
    if !subagent::is_trusted_directory(&subagents_dir) {
        return Ok(Vec::new());
    }

    let mut runs = Vec::new();
    for entry in fs::read_dir(subagents_dir)? {
        let dir = entry?.path();
        if !subagent::is_trusted_directory(&dir) {
            continue;
        }
        let Some(id) = dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|id| subagent::normalize_id(id).ok())
        else {
            continue;
        };
        let state = subagent::read_status(&dir.join(RESULT_FILE_NAME)).map(|status| status.state);
        runs.push(LinkedRun {
            dir,
            id,
            state,
            cleanup: RunCleanup::Structural,
        });
    }
    Ok(runs)
}

fn find_parent_linked_runs(
    subagents_root: &Path,
    parent_session_id: &str,
) -> anyhow::Result<Vec<LinkedRun>> {
    let mut runs = Vec::new();
    if !subagent::is_trusted_directory(subagents_root) {
        return Ok(runs);
    }
    for entry in fs::read_dir(subagents_root)? {
        let dir = entry?.path();
        if !subagent::is_trusted_directory(&dir) {
            continue;
        }
        let Some(id) = dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|id| subagent::normalize_id(id).ok())
        else {
            continue;
        };
        let status_path = dir.join(RESULT_FILE_NAME);
        let Some(status) = subagent::read_status(&status_path) else {
            continue;
        };
        if status.parent_session_id.as_deref() != Some(parent_session_id) {
            continue;
        }
        runs.push(LinkedRun {
            dir,
            id,
            state: Some(status.state),
            cleanup: RunCleanup::Explicit,
        });
    }
    Ok(runs)
}

pub(super) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// True when the session belongs to a different workspace than `cwd`.
pub fn is_cross_project(session_cwd: &Path, cwd: &Path) -> bool {
    workspace_key(session_cwd) != workspace_key(cwd)
}

#[cfg(test)]
pub(super) fn write_linked_run_for_tests(
    subagents_root: &Path,
    run_id: &str,
    parent_session_id: &str,
    state: RunState,
) -> PathBuf {
    let dir = subagents_root.join(run_id);
    fs::create_dir_all(&dir).unwrap();
    let status = RunStatus {
        state,
        parent_session_id: Some(parent_session_id.to_string()),
        agent_id: Some("worker".into()),
        ..RunStatus::default()
    };
    subagent::initialize_status(&dir.join(RESULT_FILE_NAME), &status).unwrap();
    dir
}

#[cfg(test)]
#[path = "delete_tests.rs"]
mod tests;
