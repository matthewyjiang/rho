//! A session's git worktree (#1673): the daemon adds it before the session
//! opens (see `crate::worktree`), and names it in the error of a session
//! that then fails to open, since it stays.
//!
//! Cleanup removes a worktree only when nothing could lose by it: no session
//! loaded in it and no job in it under way, no uncommitted or untracked
//! change, and what it has checked out merged into its origin's `HEAD`. It
//! keeps the branch, and a session reopened later gets its worktree back
//! from it. A pass runs at the daemon's start, once a day, and on
//! `rho/worktree/prune`; an unsaved session's worktree is checked as the
//! session is discarded, since nothing would remember it after. Every
//! check and removal of one worktree holds its session's reopen lock, so
//! the session cannot load meanwhile.
//!
//! A worktree kept for [`STALE_AFTER`] without use is stale: clients flag
//! it, and only a person removes it.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use rho_protocol::{ErrorCode, RpcError, SessionWorktree, WorktreeReport, WorktreeVerdict};

use super::{open::Workspace, Sessions};
use crate::{
    app::daemon_host::jobs::{unix_now, JobRecord, JobSubject},
    worktree::Holds,
};

/// How long a worktree cleanup keeps sits unused before it is stale: no
/// change to its session's job, nothing loaded or under way in it.
//
// Policy, not a measurement: the cleanup proposal Matt approved in #1917.
pub(in crate::app::daemon_host) const STALE_AFTER: Duration =
    Duration::from_secs(14 * 24 * 60 * 60);

/// Whether a cleanup pass removes the worktrees it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) enum Prune {
    Remove,
    /// Only report what it would remove.
    DryRun,
}

/// The workspaces the daemon's jobs use now: those of loaded sessions and
/// of jobs under way. A worktree holding one is in use.
pub(in crate::app::daemon_host) struct InUse(Vec<PathBuf>);

impl InUse {
    fn holds(&self, worktree: &Path) -> bool {
        self.0
            .iter()
            .any(|workspace| workspace.starts_with(worktree))
    }
}

/// Adds a worktree under `root` for a session asked to open in
/// `workspace`: the workspace in it the session opens in, and the worktree.
pub(super) async fn add_for_session(
    root: &Path,
    workspace: &Workspace,
) -> Result<(Workspace, SessionWorktree), RpcError> {
    let added = crate::worktree::add(root, workspace.path())
        .await
        .map_err(|error| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!("could not open session in a worktree: {error}"),
            )
        })?;
    tracing::info!(
        branch = %added.worktree.branch,
        path = %added.worktree.path.display(),
        "added a session worktree"
    );
    let workspace =
        Workspace::parse(&added.cwd).map_err(|error| kept(error, Some(&added.worktree)))?;
    Ok((workspace, added.worktree))
}

/// `error`, of a session that failed to open, naming the worktree added for
/// it, which stays.
pub(super) fn kept(mut error: RpcError, worktree: Option<&SessionWorktree>) -> RpcError {
    if let Some(worktree) = worktree {
        error.message = format!(
            "{}; the worktree added for it stays at {} on branch {}",
            error.message,
            worktree.path.display(),
            worktree.branch
        );
    }
    error
}

/// Adds back the worktree of `record`, a session's job about to reopen, if
/// a cleanup removed it, from its kept branch, so the session works where
/// it did.
pub(super) async fn restore(record: &JobRecord) -> Result<(), RpcError> {
    let Some(worktree) = &record.worktree else {
        return Ok(());
    };
    if worktree.path.exists() {
        return Ok(());
    }
    crate::worktree::restore(worktree, &record.workspace)
        .await
        .map_err(|error| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!("could not add back the session's worktree: {error}"),
            )
        })?;
    tracing::info!(
        branch = %worktree.branch,
        path = %worktree.path.display(),
        "restored a session worktree"
    );
    Ok(())
}

/// Whether a session job that last changed at `updated_at`, Unix seconds,
/// has gone unused long enough at `now` for its kept worktree to be stale.
fn stale_at(updated_at: u64, now: u64) -> bool {
    now.saturating_sub(updated_at) >= STALE_AFTER.as_secs()
}

/// Whether `record`'s worktree is stale at `now`: still on disk, not
/// `in_use`, and its job unchanged for [`STALE_AFTER`].
pub(in crate::app::daemon_host) fn worktree_stale(
    record: &JobRecord,
    in_use: &InUse,
    now: u64,
) -> bool {
    record.worktree.as_ref().is_some_and(|worktree| {
        stale_at(record.updated_at, now) && !in_use.holds(&worktree.path) && worktree.path.exists()
    })
}

impl Sessions {
    /// The workspaces `records`, every job, use now (see [`InUse`]).
    pub(in crate::app::daemon_host) fn worktrees_in_use(&self, records: &[JobRecord]) -> InUse {
        let loaded = self.loaded();
        InUse(
            records
                .iter()
                .filter(|record| {
                    record.state.in_progress()
                        || matches!(&record.subject, JobSubject::Session { session } if loaded.contains_key(session))
                })
                .map(|record| record.workspace.clone())
                .collect(),
        )
    }

    /// Runs one cleanup pass over every session worktree the daemon knows,
    /// and reports each that was on disk.
    pub(in crate::app::daemon_host) async fn prune_worktrees(
        &self,
        prune: Prune,
    ) -> Vec<WorktreeReport> {
        // One record per worktree: a session's latest job holds it.
        let mut latest: HashMap<PathBuf, JobRecord> = HashMap::new();
        let mut order = Vec::new();
        for record in self.jobs.list() {
            let Some(worktree) = &record.worktree else {
                continue;
            };
            if !latest.contains_key(&worktree.path) {
                order.push(worktree.path.clone());
            }
            latest.insert(worktree.path.clone(), record);
        }
        let mut reports = Vec::new();
        for path in order {
            let Some(record) = latest.remove(&path) else {
                continue;
            };
            let JobSubject::Session { session } = &record.subject else {
                continue;
            };
            let _reopening = self.reopening.lock(session).await;
            reports.extend(self.prune_worktree(&record, prune).await);
        }
        let removed = reports
            .iter()
            .filter(|report| matches!(report.verdict, WorktreeVerdict::Merged { .. }))
            .count();
        tracing::info!(
            ?prune,
            removed,
            kept = reports.len() - removed,
            "pruned session worktrees"
        );
        reports
    }

    /// Checks `record`'s worktree, and removes it as `prune` says when it
    /// may; `None` when it is not on disk. The caller holds the session's
    /// reopen lock, so the session cannot load meanwhile.
    pub(super) async fn prune_worktree(
        &self,
        record: &JobRecord,
        prune: Prune,
    ) -> Option<WorktreeReport> {
        let worktree = record.worktree.as_ref()?;
        // Read under the lock: an earlier pass may have removed it.
        if !worktree.path.exists() {
            return None;
        }
        let in_use = self.worktrees_in_use(&self.jobs.list());
        let verdict = match in_use.holds(&worktree.path) {
            true => WorktreeVerdict::InUse,
            false => match crate::worktree::inspect(worktree).await {
                Ok(Holds::Changes) => WorktreeVerdict::Dirty,
                Ok(Holds::Unmerged { base }) => WorktreeVerdict::Unmerged { base },
                Ok(Holds::Merged { base }) => match prune {
                    Prune::DryRun => WorktreeVerdict::Merged { base },
                    Prune::Remove => match crate::worktree::remove(worktree).await {
                        Ok(()) => {
                            tracing::info!(
                                branch = %worktree.branch,
                                path = %worktree.path.display(),
                                %base,
                                "removed a merged session worktree"
                            );
                            WorktreeVerdict::Merged { base }
                        }
                        Err(error) => WorktreeVerdict::Failed {
                            error: error.to_string(),
                        },
                    },
                },
                Err(error) => WorktreeVerdict::Failed {
                    error: error.to_string(),
                },
            },
        };
        let stale = !matches!(
            verdict,
            WorktreeVerdict::InUse | WorktreeVerdict::Merged { .. }
        ) && stale_at(record.updated_at, unix_now());
        Some(WorktreeReport {
            job: (&record.id).into(),
            worktree: worktree.clone(),
            verdict,
            stale,
        })
    }
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
