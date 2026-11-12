//! Cleaning up session worktrees (#1673): the daemon removes a session's
//! worktree once nothing uses it, it holds no uncommitted or untracked
//! change, and its branch is merged. It keeps the branch.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{JobId, SessionWorktree};

/// Remove every session worktree the daemon knows that is safe to remove,
/// and report what it did with each one still on disk. The daemon runs the
/// same pass when it starts and once a day.
pub const WORKTREE_PRUNE: &str = "rho/worktree/prune";

/// Params of [`WORKTREE_PRUNE`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreePruneParams {
    /// Report what would be removed without removing anything.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

/// Result of [`WORKTREE_PRUNE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreePruneResponse {
    /// Each session worktree that was on disk when the pass reached it, in
    /// the order the daemon lists jobs.
    pub worktrees: Vec<WorktreeReport>,
}

/// One session worktree, and what [`WORKTREE_PRUNE`] did with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeReport {
    /// The job of the session the worktree was added for.
    pub job: JobId,
    pub worktree: SessionWorktree,
    pub verdict: WorktreeVerdict,
    /// Whether the worktree is stale, as
    /// [`super::JobSummary::worktree_stale`] says.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

/// What [`WORKTREE_PRUNE`] found a worktree to be. Only a
/// [`Self::Merged`] one is removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorktreeVerdict {
    /// Clean, and its branch is merged into `base`, the current `HEAD` of
    /// the checkout it was added from: removed with its branch kept, or, in
    /// a dry run, would be.
    Merged { base: String },
    /// Its session is loaded, or a job in it is under way.
    InUse,
    /// It holds uncommitted or untracked changes.
    Dirty,
    /// Its branch has commits `base` does not.
    Unmerged { base: String },
    /// Git could not check or remove it, saying `error`.
    Failed { error: String },
    /// A verdict a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
