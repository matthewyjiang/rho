//! Git workspace identity for session paths: which checkout and which
//! repository a directory belongs to. Shared by session search scopes and the
//! `/sessions` hub grouping.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Workspace {
    /// Checkout root: the directory holding `.git`, or the directory itself
    /// outside Git.
    pub(crate) worktree: PathBuf,
    /// Shared Git directory, identical for every worktree of one repository.
    /// Equals `worktree` outside Git.
    pub(crate) repo: PathBuf,
}

impl Workspace {
    /// Whether the directory sits inside a Git checkout.
    pub(crate) fn is_git(&self) -> bool {
        self.repo != self.worktree
    }

    /// Resolve Git administrative paths without running project configuration,
    /// hooks or subprocesses. A non-Git directory is its own local scope.
    pub(crate) fn resolve(cwd: &Path) -> Self {
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        for ancestor in cwd.ancestors() {
            let dot_git = ancestor.join(".git");
            let git_dir = if dot_git.is_dir() {
                dot_git
            } else if let Ok(contents) = std::fs::read_to_string(&dot_git) {
                let Some(path) = contents.trim().strip_prefix("gitdir: ") else {
                    continue;
                };
                ancestor.join(path)
            } else {
                continue;
            };
            let common = std::fs::read_to_string(git_dir.join("commondir"))
                .map(|relative| git_dir.join(relative.trim()))
                .unwrap_or(git_dir);
            if let Ok(repo) = common.canonicalize() {
                return Self {
                    worktree: ancestor.to_path_buf(),
                    repo,
                };
            }
        }
        Self {
            worktree: cwd.clone(),
            repo: cwd,
        }
    }
}
