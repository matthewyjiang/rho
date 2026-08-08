//! Session delete with cascade cleanup of parent-linked subagent runs.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(test)]
use crate::subagent::RunStatus;
use crate::subagent::{self, RunState, RESULT_FILE_NAME};

use super::{
    index,
    persistence::{
        read_session_cwd, session_dir_in_root, workspace_key, ResolvedSession, SessionStore,
        SessionUnit,
    },
    SessionSummary,
};

/// Controls for [`super::Session::delete_by_id`].
#[derive(Clone, Debug, Default)]
pub struct DeleteOptions {
    /// When true, delete even if a parent-linked run is still non-terminal.
    ///
    /// Only intended for stale `Running`/`Starting` artifacts left behind after a
    /// crash. Live runs may still be writing; prefer waiting for completion.
    pub force: bool,
    /// Refuse delete when the resolved session id equals this value.
    pub protect_session_id: Option<String>,
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

pub(super) fn list_all_in_root(session_root: &Path) -> anyhow::Result<Vec<SessionSummary>> {
    let mut summaries = Vec::new();
    if !session_root.is_dir() {
        return Ok(summaries);
    }
    for entry in fs::read_dir(session_root)? {
        let workspace_dir = entry?.path();
        if !workspace_dir.is_dir() {
            continue;
        }
        summaries.extend(list_workspace_dir(session_root, &workspace_dir)?);
    }
    summaries.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(summaries)
}

/// Lists one workspace directory through the same synced index the resume
/// picker uses, so unchanged transcripts are never re-parsed.
///
/// The directory name is a one-way hash of the workspace cwd, so recover the
/// cwd from a session header line (first line only). When no header
/// round-trips to this directory, fall back to parsing each transcript so
/// damaged or legacy workspaces still list.
fn list_workspace_dir(
    session_root: &Path,
    workspace_dir: &Path,
) -> anyhow::Result<Vec<SessionSummary>> {
    for entry in fs::read_dir(workspace_dir)? {
        let path = entry?.path();
        let Some(unit) = SessionUnit::from_path(&path) else {
            continue;
        };
        let Ok(cwd) = read_session_cwd(&unit.transcript_path()) else {
            continue;
        };
        if session_dir_in_root(session_root, &cwd) != workspace_dir {
            continue;
        }
        return SessionStore::new(session_root, &cwd).list();
    }

    let mut summaries = Vec::new();
    for entry in fs::read_dir(workspace_dir)? {
        let path = entry?.path();
        let Some(unit) = SessionUnit::from_path(&path) else {
            continue;
        };
        match super::persistence::summarize_session_file(&unit.transcript_path(), Path::new("")) {
            Ok(record) => summaries.push(record.summary),
            Err(_) => continue,
        }
    }
    Ok(summaries)
}

pub(super) fn delete_in_roots(
    session_root: &Path,
    subagents_root: &Path,
    cwd: &Path,
    id_prefix: &str,
    options: &DeleteOptions,
) -> anyhow::Result<DeleteOutcome> {
    let resolved = SessionStore::new(session_root, cwd).resolve(id_prefix)?;
    delete_resolved(session_root, subagents_root, resolved, options)
}

fn delete_resolved(
    session_root: &Path,
    subagents_root: &Path,
    resolved: ResolvedSession,
    options: &DeleteOptions,
) -> anyhow::Result<DeleteOutcome> {
    if options
        .protect_session_id
        .as_deref()
        .is_some_and(|protected| protected == resolved.id)
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

    let parent_session_id = resolved.id.clone();
    let cleanup_guard = subagent::lock_parent_for_cleanup(subagents_root, &parent_session_id)?;

    let mut linked = find_nested_runs(&unit)?;
    linked.extend(find_parent_linked_runs(subagents_root, &resolved.id)?);
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
            let crash_hint = if matches!(run.state, Some(RunState::Running | RunState::Starting)) {
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

    // Delete session bytes first so nested artifacts disappear with the folder
    // before index rows for this parent are cleared.
    unit.delete_from_disk()?;
    index::remove_session(session_root, &workspace_key(&resolved.cwd), &resolved.id)?;

    let deleted_run_count = linked.len();
    for run in linked
        .into_iter()
        .filter(|run| run.cleanup == RunCleanup::Explicit)
    {
        match fs::remove_dir_all(&run.dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "deleted session '{}' but failed to remove related run {}: {error}",
                    resolved.id,
                    run.id
                ));
            }
        }
    }

    cleanup_guard.clear_index_and_unlock()?;

    Ok(DeleteOutcome {
        id: resolved.id,
        cwd: resolved.cwd,
        path: resolved.path,
        deleted_run_count,
        forced_run_ids,
    })
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
