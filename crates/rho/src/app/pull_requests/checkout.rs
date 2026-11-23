//! What a session's workspace has checked out, asked of `git` when a
//! client lists or shows the session: the branch whose pull request the
//! session shows, and the repository `#N` names.

use std::{
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

use futures_util::{stream, StreamExt as _};
use tokio::process::Command;

use super::model::{BranchId, RepoId};

/// How long one `git` command here may take. Each reads local files:
/// `symbolic-ref` and `remote get-url origin` took 3 to 4 ms on 2026-11-23
/// in rho's own worktree. A tripwire for a hung filesystem: past it the
/// daemon logs a warning naming the command and this deadline, and the
/// workspace reads as no repository, or as one with no origin.
const GIT_DEADLINE: Duration = Duration::from_secs(5);

/// The most workspaces [`resolve_each`] asks `git` about at once.
///
/// Receipt: each workspace takes two `git` commands of 3 to 4 ms
/// (2026-11-23), so a sidebar of 30 sessions resolves in about 30 ms at 8 at
/// a time, against 240 ms one at a time, without starting 60 processes at
/// once. Chosen; a larger list only takes longer.
const RESOLVES_AT_ONCE: usize = 8;

/// What a workspace has checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Checkout {
    /// Not in a git repository, or `git` did not answer.
    NotGit,
    /// No branch: a commit, abbreviated as `git` abbreviates it.
    Detached { commit: String },
    /// A branch of a repository with no `origin`, or one on no forge.
    NoForge { branch: String },
    /// A branch of the repository `origin` names on its forge.
    Branch(BranchId),
}

impl Checkout {
    /// The repository `origin` names, which resolves `#N`; `None` when the
    /// checkout is on no branch of a forge's repository.
    pub(super) fn repo(&self) -> Option<&RepoId> {
        match self {
            Self::Branch(branch) => Some(&branch.repo),
            Self::NotGit | Self::Detached { .. } | Self::NoForge { .. } => None,
        }
    }
}

/// What `workspace` has checked out: `git symbolic-ref --short -q HEAD`
/// names the branch (exit 1 means none, and `rev-parse --short HEAD` names
/// the commit; anything else, no repository), and `origin` its repository.
pub(crate) async fn resolve(workspace: &Path) -> Checkout {
    let Some(head) = git(workspace, &["symbolic-ref", "--short", "-q", "HEAD"]).await else {
        return Checkout::NotGit;
    };
    match head.status.code() {
        Some(0) => {}
        Some(1) => {
            let commit = git(workspace, &["rev-parse", "--short", "HEAD"]).await;
            return match commit {
                Some(commit) if commit.status.success() => Checkout::Detached {
                    commit: stdout(&commit),
                },
                Some(_) | None => Checkout::NotGit,
            };
        }
        Some(_) | None => return Checkout::NotGit,
    }
    let branch = stdout(&head);
    match origin(workspace).await {
        Some(repo) => Checkout::Branch(BranchId { repo, branch }),
        None => Checkout::NoForge { branch },
    }
}

/// What each workspace has checked out, in order, [`RESOLVES_AT_ONCE`] at
/// a time; `None` where there is no workspace.
pub(crate) async fn resolve_each(workspaces: Vec<Option<PathBuf>>) -> Vec<Option<Checkout>> {
    stream::iter(workspaces)
        .map(|workspace| async move {
            match workspace {
                Some(workspace) => Some(resolve(&workspace).await),
                None => None,
            }
        })
        .buffered(RESOLVES_AT_ONCE)
        .collect()
        .await
}

/// The repository `workspace`'s `origin` remote names on a forge, if git
/// answers in time.
pub(super) async fn origin(workspace: &Path) -> Option<RepoId> {
    let output = git(workspace, &["remote", "get-url", "origin"]).await?;
    if !output.status.success() {
        return None;
    }
    RepoId::from_url(std::str::from_utf8(&output.stdout).ok()?)
}

/// `git -C workspace args`, within [`GIT_DEADLINE`]; `None` when it could
/// not start or did not finish in time.
async fn git(workspace: &Path, args: &[&str]) -> Option<Output> {
    let mut command = Command::new("git");
    for name in crate::git::LOCATION_ENV {
        command.env_remove(name);
    }
    command
        .arg("-C")
        .arg(workspace)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(GIT_DEADLINE, command.output()).await {
        Ok(output) => output.ok(),
        Err(_) => {
            tracing::warn!(
                workspace = %workspace.display(),
                ?args,
                deadline_secs = GIT_DEADLINE.as_secs(),
                "git did not answer in time; reading the workspace's checkout without it"
            );
            None
        }
    }
}

/// `output`'s stdout, trimmed.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[cfg(test)]
#[path = "checkout_tests.rs"]
mod tests;
