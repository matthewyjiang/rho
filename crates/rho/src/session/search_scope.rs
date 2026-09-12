use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Scope {
    #[default]
    Repo,
    Worktree,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Workspace {
    pub worktree: PathBuf,
    pub repo: PathBuf,
}

impl Workspace {
    /// Resolve Git administrative paths without running project configuration,
    /// hooks or subprocesses. A non-Git directory is its own local scope.
    pub fn resolve(cwd: &Path) -> Self {
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
