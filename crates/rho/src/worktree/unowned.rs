//! Worktrees under rho's worktree directory that no daemon session owns:
//! those `rho run --worktree` added in the foreground, and those an agent
//! or a person added there with `git worktree add`. Cleanup checks them as
//! it does a session's, without a session to give one back to.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use rho_protocol::SessionWorktree;

use super::{canonical, succeeded, WorktreeError};

/// A worktree no daemon session owns.
#[derive(Debug)]
pub(crate) struct Unowned {
    /// Its branch, or its commit when [`Self::detached`], and the
    /// repository's main checkout as its origin.
    pub(crate) worktree: SessionWorktree,
    /// Whether it has a commit checked out on no branch.
    pub(crate) detached: bool,
    /// When its checkout last moved, by a commit, checkout, or reset in it.
    pub(crate) moved_at: Option<SystemTime>,
}

/// Every git worktree directly under `root` but those in `owned`, which
/// are canonical. A directory that is no linked worktree, as a clone is
/// not, stays out, and so does one git cannot read, with a warning.
pub(crate) async fn unowned(root: &Path, owned: &[&Path]) -> Vec<Unowned> {
    let Ok(root) = canonical(root) else {
        // No worktree was ever added.
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // Not followed: a symlink there is none of rho's.
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        // A linked worktree's `.git` is a file naming its admin directory.
        if !is_dir || !path.join(".git").is_file() || owned.contains(&path.as_path()) {
            continue;
        }
        match read(path.clone()).await {
            Ok(unowned) => found.push(unowned),
            Err(error) => tracing::warn!(
                path = %path.display(),
                %error,
                "could not read a worktree no session owns; leaving it"
            ),
        }
    }
    found
}

async fn read(path: PathBuf) -> Result<Unowned, WorktreeError> {
    let malformed = |detail: &str| WorktreeError::Git {
        action: "worktree list".to_owned(),
        repository: path.clone(),
        detail: detail.to_owned(),
    };
    let admin = succeeded(&path, "rev-parse", ["rev-parse", "--absolute-git-dir"]).await?;
    // One block per worktree, the main one first: `worktree <path>`, then
    // `HEAD <commit>`, then `branch refs/heads/<name>` or `detached`.
    let listed = succeeded(&path, "worktree list", ["worktree", "list", "--porcelain"]).await?;
    let mut blocks = listed.split("\n\n").map(Entry::parse);
    let origin = blocks
        .next()
        .and_then(|main| main.path)
        .ok_or_else(|| malformed("git named no main worktree"))?;
    let this = blocks
        .find(|entry| {
            entry
                .path
                .as_deref()
                .is_some_and(|listed| canonical(listed).is_ok_and(|listed| listed == path))
        })
        .ok_or_else(|| malformed("git does not list it"))?;
    let (branch, detached) = match (this.branch, this.head) {
        (Some(branch), _) => (branch.to_owned(), false),
        (None, Some(head)) => (head.chars().take(12).collect(), true),
        (None, None) => return Err(malformed("git names neither its branch nor its commit")),
    };
    let admin = Path::new(&admin);
    let moved_at = [admin.join("HEAD"), admin.join("logs/HEAD")]
        .iter()
        .filter_map(|file| {
            std::fs::metadata(file)
                .and_then(|meta| meta.modified())
                .ok()
        })
        .max();
    Ok(Unowned {
        worktree: SessionWorktree {
            branch,
            path,
            origin,
        },
        detached,
        moved_at,
    })
}

/// One worktree as `git worktree list --porcelain` lists it.
struct Entry<'a> {
    path: Option<PathBuf>,
    head: Option<&'a str>,
    branch: Option<&'a str>,
}

impl<'a> Entry<'a> {
    fn parse(block: &'a str) -> Self {
        let field = |name: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        };
        Self {
            path: field("worktree").map(PathBuf::from),
            head: field("HEAD"),
            branch: field("branch")
                .map(|branch| branch.strip_prefix("refs/heads/").unwrap_or(branch)),
        }
    }
}
