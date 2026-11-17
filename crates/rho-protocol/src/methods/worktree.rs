//! Cleaning up session worktrees (#1673): the daemon removes a session's
//! worktree once nothing uses it, it holds no uncommitted or untracked
//! change, and its branch is merged. It keeps the branch. It does the same
//! with the other worktrees in its worktree directory, such as those
//! `rho run --worktree` adds in the foreground, once no process works in
//! one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{JobId, SessionWorktree};

/// Remove every worktree in the daemon's worktree directory that is safe to
/// remove, and report what it did with each one still on disk. The daemon
/// runs the same pass a minute after it starts and once a day, there
/// skipping worktrees used within the last day.
pub const WORKTREE_PRUNE: &str = "rho/worktree/prune";

/// Params of [`WORKTREE_PRUNE`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreePruneParams {
    /// Report what would be removed without removing anything.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// Also measure the space each worktree takes
    /// ([`WorktreeReport::bytes`]), which walks every file in it; only a
    /// dry run measures.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub measure: bool,
}

/// Result of [`WORKTREE_PRUNE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreePruneResponse {
    /// Each worktree that was on disk when the pass reached it: the
    /// sessions' in the order the daemon lists jobs, then the others.
    pub worktrees: Vec<WorktreeReport>,
}

/// One worktree, and what [`WORKTREE_PRUNE`] did with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeReport {
    /// The job of the session the worktree was added for; absent for one
    /// no daemon session owns. For that one, `worktree` names the branch
    /// checked out, or the commit when [`Self::detached`], and as its
    /// `origin` the repository's main checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobId>,
    pub worktree: SessionWorktree,
    /// Whether the worktree has a commit checked out alone, on no branch:
    /// `worktree.branch` names the commit, and removing it keeps no branch.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub detached: bool,
    pub verdict: WorktreeVerdict,
    /// Whether the worktree is stale, as
    /// [`super::JobSummary::worktree_stale`] says.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    /// The bytes of the files in the worktree, each file linked more than
    /// once counted once: about what removing it frees. Only a dry run
    /// asked to [`WorktreePruneParams::measure`] measures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// What [`WORKTREE_PRUNE`] found a worktree to be. Only a
/// [`Self::Merged`] one is removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorktreeVerdict {
    /// Clean, and its branch is merged into `base`, the current `HEAD` of
    /// the checkout it was added from: removed with its branch kept, or, in
    /// a dry run, would be. A [`WorktreeReport::detached`] one has no branch
    /// to keep: `base` has its changes, but maybe not its commits.
    Merged { base: String },
    /// Its session is loaded, or a job in it is under way.
    InUse,
    /// No daemon session owns it, and a process works in it, such as a
    /// shell or a foreground `rho run --worktree`.
    Occupied,
    /// No daemon session owns it, and its checkout moved, or a job in it
    /// changed, within the last day, as one a session is still opening in,
    /// or an agent just added, has.
    Recent,
    /// It holds uncommitted or untracked changes.
    Dirty,
    /// Its branch has commits `base` does not.
    Unmerged { base: String },
    /// Rho could not check or remove it, saying `error`: git failed, as for
    /// a worktree `git worktree lock` locked, or, for one no session owns,
    /// rho cannot see which processes work in it on this platform.
    Failed { error: String },
    /// A verdict a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl WorktreeVerdict {
    /// Whether the pass removes a worktree with this verdict, or in a dry
    /// run would.
    pub fn removes(&self) -> bool {
        matches!(self, Self::Merged { .. })
    }
}
