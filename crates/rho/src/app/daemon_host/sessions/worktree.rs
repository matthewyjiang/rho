//! A session's git worktree (#1673): the daemon adds it before the session
//! opens (see `crate::worktree`), and names it in the error of a session
//! that then fails to open, since it stays.
//!
//! Cleanup removes a worktree only when nothing could lose by it: no session
//! loaded in it and no job in it under way, no uncommitted or untracked
//! change, and what it has checked out merged into its origin's `HEAD`. It
//! keeps the branch and records the commit it removed. A session reopened
//! later gets its worktree back on that branch, made again at that commit
//! when someone deleted it since. Reading the session does the same, or
//! checks the commit out alone when the branch is checked out elsewhere,
//! so a read never fails on the branch. A worktree a person removed has no
//! commit recorded, and the daemon never adds it back.
//!
//! The daemon's own pass runs at its start and once a day, and skips
//! worktrees whose session changed within [`QUIET_BEFORE_DAILY`], so a
//! session a client reconnects to after a restart keeps its worktree and
//! the files git ignores there. `rho/worktree/prune` checks every worktree
//! at once. An unsaved session's worktree is checked as the session is
//! discarded, since nothing would remember it after. Every check and
//! removal of one worktree holds its session's reopen lock, so the session
//! cannot load meanwhile.
//!
//! A worktree that stays on disk unused for [`STALE_AFTER`] is stale:
//! clients flag it, and only a person removes it.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use rho_protocol::{ErrorCode, RpcError, SessionWorktree, WorktreeReport, WorktreeVerdict};

use super::{open::Workspace, Sessions};
use crate::{
    app::daemon_host::jobs::{unix_now, JobRecord, JobSubject},
    worktree::{Checkout, Holds},
};

/// How long a worktree sits on disk unused before it is stale: no change
/// to its session's job, nothing loaded or under way in it.
//
// Policy, not a measurement: the cleanup proposal Matt approved in #1917.
const STALE_AFTER: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How long a session's job must be unchanged before the daemon's own pass
/// removes its worktree.
//
// Policy: as long as the pass's period, so a worktree goes within two days
// of its last use. It only has to outlast a client reconnecting after a
// restart or upgrade, which takes seconds; the daily pass makes anything
// shorter than a day remove nothing sooner.
const QUIET_BEFORE_DAILY: Duration = Duration::from_secs(24 * 60 * 60);

/// What a cleanup pass does with the worktrees it may remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) enum Prune {
    /// The daemon's own pass: removes those whose session has been quiet
    /// for [`QUIET_BEFORE_DAILY`], and skips the rest unreported.
    Daily,
    /// A person's request: removes them all now.
    Now,
    /// Only reports what [`Self::Now`] would remove.
    DryRun,
}

impl Prune {
    /// Whether this pass checks the worktree of a job last changed at
    /// `updated_at`, Unix seconds, at `now`.
    fn checks(self, updated_at: u64, now: u64) -> bool {
        match self {
            Self::Daily => now.saturating_sub(updated_at) >= QUIET_BEFORE_DAILY.as_secs(),
            Self::Now | Self::DryRun => true,
        }
    }
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

/// Whether `record`'s worktree is stale at `now`, Unix seconds: still on
/// disk, not `in_use`, and its job unchanged for [`STALE_AFTER`]. The one
/// rule `rho/job/list` and `rho/worktree/prune` both report.
pub(in crate::app::daemon_host) fn worktree_stale(
    record: &JobRecord,
    in_use: &InUse,
    now: u64,
) -> bool {
    record.worktree.as_ref().is_some_and(|worktree| {
        now.saturating_sub(record.updated_at) >= STALE_AFTER.as_secs()
            && !in_use.holds(&worktree.path)
            && worktree.path.exists()
    })
}

/// What a session whose worktree cleanup removed is about to be opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Restore {
    /// Turns, which work on its branch.
    Turns,
    /// Reading its transcript, which only needs the directory.
    Reading,
}

impl Sessions {
    /// Adds back the worktree of `record`, the job of a session about to be
    /// opened for `purpose`, if the daemon's cleanup removed it, at the
    /// commit it removed, so the session works where it did. The caller
    /// holds the session's reopen lock.
    pub(super) async fn restore_worktree(
        &self,
        record: &mut JobRecord,
        purpose: Restore,
    ) -> Result<(), RpcError> {
        let (Some(worktree), Some(commit)) = (&record.worktree, &record.worktree_removed_at) else {
            return Ok(());
        };
        let restore =
            |checkout| crate::worktree::restore(worktree, commit, &record.workspace, checkout);
        let restored = match (restore(Checkout::Branch).await, purpose) {
            (Ok(()), _) => Ok(Checkout::Branch),
            (Err(error), Restore::Reading) => {
                tracing::warn!(
                    branch = %worktree.branch,
                    %error,
                    "could not add back a session worktree on its branch; checking its commit out alone to read it"
                );
                restore(Checkout::Detached)
                    .await
                    .map(|()| Checkout::Detached)
            }
            (Err(error), Restore::Turns) => Err(error),
        };
        let checkout = restored.map_err(|error| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!(
                    "could not add back the session's worktree at {} on branch {}: {error}",
                    worktree.path.display(),
                    worktree.branch
                ),
            )
        })?;
        tracing::info!(
            branch = %worktree.branch,
            path = %worktree.path.display(),
            %commit,
            ?checkout,
            "restored a session worktree"
        );
        record.worktree_removed_at = None;
        self.jobs.set_worktree_removed_at(&record.id, None);
        Ok(())
    }

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
        // Only a new session's job gets a worktree, and a reopen reuses
        // that job, so one record holds each.
        let mut reports = Vec::new();
        for listed in self.jobs.list() {
            let (Some(_), JobSubject::Session { session }) = (&listed.worktree, &listed.subject)
            else {
                continue;
            };
            let _reopening = self.reopening.lock(session).await;
            // Read again under the lock: the session may have run since.
            let Some(record) = self.jobs.get(&listed.id) else {
                continue;
            };
            if !prune.checks(record.updated_at, unix_now()) {
                continue;
            }
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
                Ok(Holds::Merged { base, commit }) => match prune {
                    Prune::DryRun => WorktreeVerdict::Merged { base },
                    Prune::Daily | Prune::Now => match crate::worktree::remove(worktree).await {
                        Ok(()) => {
                            self.jobs
                                .set_worktree_removed_at(&record.id, Some(commit.clone()));
                            tracing::info!(
                                branch = %worktree.branch,
                                path = %worktree.path.display(),
                                %base,
                                %commit,
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
        // A removed worktree is no longer on disk, so it is not stale.
        let stale = worktree_stale(record, &in_use, unix_now());
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
