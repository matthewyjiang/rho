//! Session worktrees (#1673): a session that asks for one works in a git
//! worktree of its workspace's repository, on a new branch from that
//! repository's `HEAD`, so sessions running at once in one repository never
//! write the same files. The worktree lives under the rho home's
//! `worktrees` directory, never inside the repository, and the session
//! works at its workspace's place in it.
//!
//! The daemon adds one for a session opened with
//! `SessionNewParams::worktree`, and a foreground `rho run --worktree` for
//! its own session. Nothing removes a worktree: it holds the session's work
//! until a person merges or drops it, with `git worktree remove <path>` and
//! `git branch -D <branch>` in the repository.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use rho_protocol::SessionWorktree;

/// The directory under `rho_home` that holds session worktrees.
pub(crate) fn root(rho_home: &Path) -> PathBuf {
    rho_home.join("worktrees")
}

/// A worktree added for a session, and the directory in it the session
/// works in.
#[derive(Debug)]
pub(crate) struct Added {
    pub(crate) worktree: SessionWorktree,
    pub(crate) cwd: PathBuf,
}

/// Why no worktree could be added.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WorktreeError {
    #[error(
        "{} is not in a git repository, so no worktree can be added for it",
        .0.display()
    )]
    NotARepository(PathBuf),
    #[error("could not run git: {0}")]
    NoGit(#[source] std::io::Error),
    #[error("git {action} failed in {}: {detail}", repository.display())]
    Git {
        action: &'static str,
        repository: PathBuf,
        detail: String,
    },
    #[error("could not create {}: {error}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
}

/// Adds a worktree under `root` for a session asked to work in
/// `workspace`, a canonical directory: of the repository `workspace` is in,
/// on a new branch from its `HEAD`. The worktree and its branch share one
/// name, the repository's directory name and a random suffix.
pub(crate) async fn add(root: &Path, workspace: &Path) -> Result<Added, WorktreeError> {
    let toplevel = match git(workspace, &["rev-parse", "--show-toplevel"]).await? {
        Ok(stdout) => canonical(Path::new(stdout.trim()))?,
        Err(stderr) if stderr.contains("not a git repository") => {
            return Err(WorktreeError::NotARepository(workspace.to_path_buf()));
        }
        Err(detail) => {
            return Err(WorktreeError::Git {
                action: "rev-parse",
                repository: workspace.to_path_buf(),
                detail,
            });
        }
    };
    let name = format!(
        "{}-{}",
        branch_safe(toplevel.file_name().map(|name| name.to_string_lossy())),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let branch = format!("rho/{name}");
    std::fs::create_dir_all(root).map_err(|error| WorktreeError::Io {
        path: root.to_path_buf(),
        error,
    })?;
    let path = root.join(&name);
    let mut args = vec!["worktree", "add", "-b", branch.as_str()];
    let path_arg = path.to_string_lossy();
    args.extend([path_arg.as_ref(), "HEAD"]);
    git(&toplevel, &args)
        .await?
        .map_err(|detail| WorktreeError::Git {
            action: "worktree add",
            repository: toplevel.clone(),
            detail,
        })?;
    let path = canonical(&path)?;
    // The same place in the worktree; a directory git does not track, such
    // as an empty or ignored one, is made there.
    let cwd = match workspace.strip_prefix(&toplevel) {
        Ok(inside) if !inside.as_os_str().is_empty() => {
            let cwd = path.join(inside);
            std::fs::create_dir_all(&cwd).map_err(|error| WorktreeError::Io {
                path: cwd.clone(),
                error,
            })?;
            cwd
        }
        Ok(_) | Err(_) => path.clone(),
    };
    Ok(Added {
        worktree: SessionWorktree {
            branch,
            path,
            origin: workspace.to_path_buf(),
        },
        cwd,
    })
}

/// What a run tells the person where its session works, as `rho run
/// --worktree` prints it.
pub(crate) fn hint(worktree: &SessionWorktree) -> String {
    format!(
        "the session works in worktree {} on branch {}; rho never removes it",
        worktree.path.display(),
        worktree.branch
    )
}

/// Runs `git args` in `dir`: its trimmed stdout when it succeeds, else its
/// trimmed stderr, in English, which `add` reads.
async fn git(dir: &Path, args: &[&str]) -> Result<Result<String, String>, WorktreeError> {
    let mut command = tokio::process::Command::new("git");
    for name in crate::git::LOCATION_ENV {
        command.env_remove(name);
    }
    let output = command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(WorktreeError::NoGit)?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    Ok(if output.status.success() {
        Ok(text(&output.stdout))
    } else {
        Err(text(&output.stderr))
    })
}

fn canonical(path: &Path) -> Result<PathBuf, WorktreeError> {
    crate::paths::canonicalize(path).map_err(|error| WorktreeError::Io {
        path: path.to_path_buf(),
        error,
    })
}

/// `name` as one component of a branch and directory name: ASCII letters,
/// digits, `_`, and `-`, every other character `-`.
fn branch_safe(name: Option<std::borrow::Cow<'_, str>>) -> String {
    let safe: String = name
        .unwrap_or_default()
        .chars()
        .map(|char| match char {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => char,
            _ => '-',
        })
        .collect();
    if safe.is_empty() {
        "repository".to_owned()
    } else {
        safe
    }
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
