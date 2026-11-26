//! A session's git worktree (#1673): the daemon adds it before the session
//! opens (see `crate::worktree`), and names it in the error of a session
//! that then fails to open, since it stays.
//!
//! Cleanup removes a worktree only when nothing could lose by it: no session
//! loaded in it and no job in it under way, no uncommitted or untracked
//! change, and what it has checked out merged into its origin's `HEAD` or
//! the remote's default branch. It keeps the branch and records the commit
//! it removed. A session reopened later gets its worktree back on that
//! branch, made again at that commit when someone deleted it since. Reading the session does the same, or,
//! when the branch is checked out elsewhere, checks the commit out alone
//! for the read and removes it again after, so a read never fails on the
//! branch and turns never run off it. A worktree a person removed has no
//! commit recorded, and the daemon never adds it back.
//!
//! The daemon's own pass runs a minute after its start and once a day, and skips
//! worktrees whose session changed within [`QUIET_BEFORE_DAILY`], so a
//! session a client reconnects to after a restart keeps its worktree and
//! the files git ignores there. `rho/worktree/prune` checks every worktree
//! at once. An unsaved session's worktree is checked as the session is
//! discarded, since nothing would remember it after. Every check and
//! removal of a session's worktree holds its session's reopen lock, so the
//! session cannot load meanwhile.
//!
//! A worktree that stays on disk unused for [`STALE_AFTER`] is stale:
//! clients flag it, and only a person removes it.
//!
//! The pass then checks the worktrees in the directory no job names, such
//! as a foreground `rho run --worktree`'s or one an agent added with `git
//! worktree add`, by the same rules. With no lock to hold, it keeps one
//! whose checkout moved within [`QUIET_BEFORE_DAILY`], every pass alike:
//! a session's worktree has no job naming it until the session opens, and
//! an agent works in its own between commands. It also keeps one a process
//! works in, checked again just before removing it. Nothing gives one of
//! those back once removed.

use std::{
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use rho_protocol::{ErrorCode, RpcError, SessionWorktree, WorktreeReport, WorktreeVerdict};

use super::{open::Workspace, Sessions};
use crate::{
    app::daemon_host::jobs::{unix_now, JobRecord, JobSubject},
    worktree::{Checkout, Holds, Start, Started},
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
    /// The daemon's own pass: removes those unused for
    /// [`QUIET_BEFORE_DAILY`], and skips the rest unreported.
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
/// `workspace`, from where `start` says: the workspace in it the session
/// opens in, and the worktree.
pub(super) async fn add_for_session(
    root: &Path,
    workspace: &Workspace,
    start: Start,
) -> Result<(Workspace, SessionWorktree), RpcError> {
    let added = crate::worktree::add(root, workspace.path(), start)
        .await
        .map_err(|error| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!("could not open session in a worktree: {error}"),
            )
        })?;
    let (branch, path) = (&added.worktree.branch, added.worktree.path.display());
    match &added.started {
        Started::FellBack(_) => {
            tracing::warn!(%branch, %path, from = %added.started, "added a session worktree");
        }
        Started::Head | Started::Remote(_) => {
            tracing::info!(%branch, %path, from = %added.started, "added a session worktree");
        }
    }
    let workspace =
        Workspace::parse("cwd", &added.cwd).map_err(|error| kept(error, Some(&added.worktree)))?;
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

/// What [`Sessions::restore_worktree`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub(super) enum Restored {
    /// Nothing: the daemon did not remove the worktree, or added it back.
    Nothing,
    /// Added it back on its branch, for good.
    OnBranch,
    /// Checked its commit out alone for a read, which removes it again
    /// after ([`Sessions::end_read_checkout`]); it still counts as removed.
    ForReading,
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
    ) -> Result<Restored, RpcError> {
        let (Some(worktree), Some(commit)) = (&record.worktree, &record.worktree_removed_at) else {
            return Ok(Restored::Nothing);
        };
        let failed = |error: crate::worktree::WorktreeError| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!(
                    "could not add back the session's worktree at {} on branch {}: {error}",
                    worktree.path.display(),
                    worktree.branch
                ),
            )
        };
        // A checkout a daemon added back and died before it recorded so, or
        // before a read removed it again. Removed first, refused if dirty.
        if worktree.path.exists() {
            crate::worktree::remove(worktree).await.map_err(failed)?;
        }
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
        let checkout = restored.map_err(failed)?;
        tracing::info!(
            branch = %worktree.branch,
            path = %worktree.path.display(),
            %commit,
            ?checkout,
            "restored a session worktree"
        );
        match checkout {
            Checkout::Branch => {
                record.worktree_removed_at = None;
                self.jobs.set_worktree_removed_at(&record.id, None);
                Ok(Restored::OnBranch)
            }
            Checkout::Detached => Ok(Restored::ForReading),
        }
    }

    /// Removes the checkout a read of `record`'s session had added
    /// ([`Restored::ForReading`]) once the read is done, so a later reopen
    /// adds the worktree back on its branch or fails, never running turns
    /// on the bare commit. The caller still holds the reopen lock.
    pub(super) async fn end_read_checkout(record: &JobRecord) {
        let Some(worktree) = &record.worktree else {
            return;
        };
        if let Err(error) = crate::worktree::remove(worktree).await {
            tracing::warn!(
                path = %worktree.path.display(),
                %error,
                "could not remove the checkout a read of a session added"
            );
        }
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
    /// then the others in its worktree directory, and reports each that
    /// was on disk.
    pub(in crate::app::daemon_host) async fn prune_worktrees(
        &self,
        pass: Pass,
    ) -> Vec<WorktreeReport> {
        // Only a new session's job gets a worktree, and a reopen reuses
        // that job, so one record holds each.
        let mut reports = Vec::new();
        for listed in self.jobs.list() {
            let (Some(_), JobSubject::Session { session }) = (&listed.worktree, &listed.subject)
            else {
                continue;
            };
            let reopening = self.reopening.lock(session).await;
            // Read again under the lock: the session may have run since.
            let Some(record) = self.jobs.get(&listed.id) else {
                continue;
            };
            if !pass.prune.checks(record.updated_at, unix_now()) {
                continue;
            }
            let Some(mut report) = self.prune_worktree(&record, pass.prune).await else {
                continue;
            };
            drop(reopening);
            // Walked once the session may load again: a build's output
            // takes long.
            report.bytes = pass.measure(&report.worktree).await;
            reports.push(report);
        }
        reports.extend(self.prune_unowned(pass).await);
        let removed = reports
            .iter()
            .filter(|report| report.verdict.removes())
            .count();
        tracing::info!(
            ?pass,
            removed,
            kept = reports.len() - removed,
            "pruned worktrees"
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
            false => {
                let checked = check_and_remove(worktree, prune, Guard::None).await;
                if let Some(commit) = checked.removed_at {
                    self.jobs.set_worktree_removed_at(&record.id, Some(commit));
                }
                checked.verdict
            }
        };
        // A removed worktree is no longer on disk, so it is not stale.
        let stale = worktree_stale(record, &in_use, unix_now());
        Some(WorktreeReport {
            job: Some((&record.id).into()),
            worktree: worktree.clone(),
            detached: false,
            verdict,
            stale,
            bytes: None,
        })
    }

    /// Checks each worktree in the worktree directory that no session owns
    /// (see [`crate::worktree::unowned`]), and removes it as `pass` says
    /// when it may. With no session's reopen lock to hold, it keeps one
    /// [`Guard::Unowned`] keeps, checked again just before removing it.
    async fn prune_unowned(&self, pass: Pass) -> Vec<WorktreeReport> {
        let records = self.jobs.list();
        let owned: Vec<&Path> = records
            .iter()
            .filter_map(|record| Some(record.worktree.as_ref()?.path.as_path()))
            .collect();
        let found = crate::worktree::unowned(&self.worktrees, &owned).await;
        let mut reports = Vec::new();
        for unowned in found {
            let worktree = &unowned.worktree;
            let guard = Guard::Unowned {
                sessions: self,
                path: &worktree.path,
            };
            let verdict = match (guard.verdict().await, pass.prune) {
                // The daemon's own pass skips what was used lately unreported.
                (Some(WorktreeVerdict::Recent), Prune::Daily) => continue,
                (Some(verdict), _) => verdict,
                (None, prune) => check_and_remove(worktree, prune, guard).await.verdict,
            };
            let bytes = pass.measure(worktree).await;
            reports.push(WorktreeReport {
                job: None,
                worktree: worktree.clone(),
                detached: unowned.detached,
                verdict,
                stale: false,
                bytes,
            });
        }
        reports
    }
}

/// One cleanup pass: what it does with the worktrees it may remove, and
/// whether it measures them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) struct Pass {
    pub(in crate::app::daemon_host) prune: Prune,
    /// Whether to report the bytes each worktree it keeps, or a dry run
    /// would remove, takes.
    pub(in crate::app::daemon_host) measure: bool,
}

impl Pass {
    /// The bytes `worktree` takes, when this pass measures and it is still
    /// on disk.
    async fn measure(self, worktree: &SessionWorktree) -> Option<u64> {
        match self.measure && worktree.path.exists() {
            true => Some(crate::worktree::size(&worktree.path).await),
            false => None,
        }
    }
}

/// What must still hold right before a worktree is removed.
#[derive(Clone, Copy)]
enum Guard<'a> {
    /// Nothing more: the caller holds the session's reopen lock.
    None,
    /// For one no session owns: no job uses it; neither its checkout nor a
    /// job in it changed within [`QUIET_BEFORE_DAILY`], as they have for a
    /// session still opening in it, before its job names it; and no process
    /// works in it.
    Unowned {
        sessions: &'a Sessions,
        path: &'a Path,
    },
}

impl Guard<'_> {
    /// The verdict that keeps the worktree now, if one does.
    async fn verdict(self) -> Option<WorktreeVerdict> {
        let Self::Unowned { sessions, path } = self else {
            return None;
        };
        let records = sessions.jobs.list();
        if sessions.worktrees_in_use(&records).holds(path) {
            return Some(WorktreeVerdict::InUse);
        }
        let admin = match crate::worktree::admin(path).await {
            Ok(admin) => admin,
            Err(error) => {
                let error = error.to_string();
                return Some(WorktreeVerdict::Failed { error });
            }
        };
        let moved_at = admin
            .moved_at
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());
        let used_at = records
            .iter()
            .filter(|record| record.workspace.starts_with(path))
            .map(|record| record.updated_at)
            .fold(moved_at, u64::max);
        if unix_now().saturating_sub(used_at) < QUIET_BEFORE_DAILY.as_secs() {
            return Some(WorktreeVerdict::Recent);
        }
        match crate::process_cwds::scan().await {
            Some(cwds) if cwds.iter().any(|cwd| cwd.starts_with(path)) => {
                Some(WorktreeVerdict::Occupied)
            }
            Some(_) => None,
            None => Some(WorktreeVerdict::Failed {
                error: "rho cannot see which processes work in it on this platform, \
                        so it keeps worktrees no session owns"
                    .to_owned(),
            }),
        }
    }
}

/// What [`check_and_remove`] found and did.
struct Checked {
    verdict: WorktreeVerdict,
    /// The commit it removed the worktree at, when it did.
    removed_at: Option<String>,
}

/// Inspects `worktree`, which nothing uses, and removes it as `prune`
/// says when it holds nothing unmerged and `guard` still lets it; the
/// inspection can take seconds, so the guard is checked again after.
async fn check_and_remove(worktree: &SessionWorktree, prune: Prune, guard: Guard<'_>) -> Checked {
    let kept = |verdict| Checked {
        verdict,
        removed_at: None,
    };
    let failed = |error: crate::worktree::WorktreeError| {
        kept(WorktreeVerdict::Failed {
            error: error.to_string(),
        })
    };
    let (base, commit) = match crate::worktree::inspect(worktree).await {
        Ok(Holds::Changes) => return kept(WorktreeVerdict::Dirty),
        Ok(Holds::Unmerged { base }) => return kept(WorktreeVerdict::Unmerged { base }),
        Ok(Holds::Merged { base, commit }) => (base, commit),
        Err(error) => return failed(error),
    };
    // `git worktree remove` would refuse it, so a dry run says so too.
    match crate::worktree::admin(&worktree.path).await {
        Ok(admin) if admin.locked => return kept(WorktreeVerdict::Locked),
        Ok(_) => {}
        Err(error) => return failed(error),
    }
    match prune {
        Prune::DryRun => return kept(WorktreeVerdict::Merged { base }),
        Prune::Daily | Prune::Now => {}
    }
    if let Some(verdict) = guard.verdict().await {
        return kept(verdict);
    }
    if let Err(error) = crate::worktree::remove(worktree).await {
        return failed(error);
    }
    tracing::info!(
        branch = %worktree.branch,
        path = %worktree.path.display(),
        %base,
        %commit,
        "removed a merged worktree"
    );
    Checked {
        verdict: WorktreeVerdict::Merged { base },
        removed_at: Some(commit),
    }
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
