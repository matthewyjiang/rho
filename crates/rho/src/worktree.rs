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
    ffi::OsStr,
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

/// Why no worktree could be added, or used.
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
        action: String,
        repository: PathBuf,
        detail: String,
    },
    #[error("could not create {}: {error}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
    /// A step after `git worktree add` failed, which leaves the worktree.
    #[error("{error}; the worktree stays at {} on branch {branch}", path.display())]
    Kept {
        path: PathBuf,
        branch: String,
        #[source]
        error: Box<WorktreeError>,
    },
    /// The daemon answered a request for a worktree without one, as a
    /// daemon of a build before session worktrees does: it ignores the
    /// request and opens the session in the workspace itself.
    #[error(
        "the rho daemon opened the session without a worktree, as a daemon older than this rho does; \
         run `rho daemon restart` once its jobs finish, then try again"
    )]
    DaemonIgnored,
}

/// What one git command did.
enum Ran {
    /// It succeeded, printing this, trimmed.
    Succeeded(String),
    /// It failed, saying this on stderr, trimmed.
    Failed(String),
}

/// Adds a worktree under `root` for a session asked to work in
/// `workspace`, a canonical directory: of the repository `workspace` is in,
/// on a new branch from its `HEAD`. The worktree and its branch share one
/// name, the repository's directory name and a random suffix.
pub(crate) async fn add(root: &Path, workspace: &Path) -> Result<Added, WorktreeError> {
    // In English, which the refusal below reads.
    let toplevel = match git(
        workspace,
        ["rev-parse", "--show-toplevel"],
        &[("LC_ALL", "C")],
    )
    .await?
    {
        Ran::Succeeded(stdout) => canonical(Path::new(&stdout))?,
        Ran::Failed(stderr) if stderr.contains("not a git repository") => {
            return Err(WorktreeError::NotARepository(workspace.to_path_buf()));
        }
        Ran::Failed(detail) => {
            return Err(WorktreeError::Git {
                action: "rev-parse".to_owned(),
                repository: workspace.to_path_buf(),
                detail,
            });
        }
    };
    let repository_name = toplevel.file_name().map(|name| name.to_string_lossy());
    let name = format!(
        "{}-{}",
        branch_safe(&repository_name.unwrap_or_default()),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let branch = format!("rho/{name}");
    std::fs::create_dir_all(root).map_err(|error| WorktreeError::Io {
        path: root.to_path_buf(),
        error,
    })?;
    let path = root.join(&name);
    let args: [&OsStr; 6] = [
        "worktree".as_ref(),
        "add".as_ref(),
        "-b".as_ref(),
        branch.as_ref(),
        path.as_os_str(),
        "HEAD".as_ref(),
    ];
    if let Ran::Failed(detail) = git(&toplevel, args, &[]).await? {
        // git keeps a worktree it made when only a hook after the checkout
        // failed.
        return Err(WorktreeError::Git {
            action: format!("worktree add {} on branch {branch}", path.display()),
            repository: toplevel,
            detail,
        });
    }
    let kept = |error| WorktreeError::Kept {
        path: path.clone(),
        branch: branch.clone(),
        error: Box::new(error),
    };
    let canonical_path = canonical(&path).map_err(kept)?;
    let inside = workspace.strip_prefix(&toplevel).map_err(|_| {
        kept(WorktreeError::Git {
            action: "rev-parse".to_owned(),
            repository: workspace.to_path_buf(),
            detail: format!(
                "the repository root {} does not hold it",
                toplevel.display()
            ),
        })
    })?;
    // The same place in the worktree; a directory git does not track, such
    // as an empty or ignored one, is made there.
    let cwd = match inside.as_os_str().is_empty() {
        true => canonical_path.clone(),
        false => canonical_path.join(inside),
    };
    std::fs::create_dir_all(&cwd).map_err(|error| {
        kept(WorktreeError::Io {
            path: cwd.clone(),
            error,
        })
    })?;
    Ok(Added {
        worktree: SessionWorktree {
            branch,
            path: canonical_path,
            origin: workspace.to_path_buf(),
        },
        cwd,
    })
}

/// Refuses a session opened for a request that `asked` for a worktree
/// when it came back without one (see [`WorktreeError::DaemonIgnored`]),
/// before anything runs in it.
pub(crate) fn require(asked: bool, opened: Option<&SessionWorktree>) -> Result<(), WorktreeError> {
    match (asked, opened) {
        (true, None) => Err(WorktreeError::DaemonIgnored),
        (true, Some(_)) | (false, _) => Ok(()),
    }
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

/// Runs `git args` in `dir` with `env` set.
async fn git(
    dir: &Path,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    env: &[(&str, &str)],
) -> Result<Ran, WorktreeError> {
    let mut command = tokio::process::Command::new("git");
    for name in crate::git::LOCATION_ENV {
        command.env_remove(name);
    }
    let output = command
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(WorktreeError::NoGit)?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    Ok(if output.status.success() {
        Ran::Succeeded(text(&output.stdout))
    } else {
        Ran::Failed(text(&output.stderr))
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
fn branch_safe(name: &str) -> String {
    let safe: String = name
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
