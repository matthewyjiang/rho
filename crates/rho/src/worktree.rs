//! Session worktrees (#1673): a session that asks for one works in a git
//! worktree of its workspace's repository, on a new branch, so sessions
//! running at once in one repository never write the same files. The branch
//! starts from the `origin` remote's default branch, fetched first, or from
//! the repository's `HEAD`, as `git.worktree_from_remote` says ([`Start`]).
//! The worktree lives under the rho home's `worktrees` directory, never
//! inside the repository, and the session works at its workspace's place in
//! it.
//!
//! The daemon adds one for a session opened with
//! `SessionNewParams::worktree`, and a foreground `rho run --worktree` for
//! its own session. The daemon removes one of its sessions' worktrees once
//! it is clean and its origin's `HEAD`, or the remote's default branch, has
//! what it has checked out, however it was merged ([`inspect`],
//! [`remove`]), keeping the branch, and adds it back at the commit it
//! removed when the session reopens ([`restore`]). It never removes one holding work nobody merged, and
//! nothing removes a foreground run's: a person does, with
//! `git worktree remove <path>` and `git branch -D <branch>`.

use std::{
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use rho_protocol::SessionWorktree;

use crate::{
    config::Config,
    process_tree::{ProcessTree as _, SupervisedTree},
};

/// The remote whose default branch [`Start::Remote`] starts from.
const REMOTE: &str = "origin";

/// How long [`Start::Remote`] waits on the remote, every git command that
/// asks it together, before starting from `HEAD`. Fetching one up-to-date
/// branch from GitHub took 0.2-1.0 s (rho, 28 MiB packed; rho-ui), and
/// `ls-remote --symref origin HEAD` 0.9 s, while a fetch from an unreachable host
/// was still waiting when killed at 30 s. Adding a worktree waits on the
/// remote this long at most.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a new worktree's branch starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Start {
    /// The repository's `HEAD`, whatever its checkout has.
    Head,
    /// The [`REMOTE`]'s default branch, fetched first, so the worktree has
    /// what the remote has even when the checkout is behind. `HEAD` when the
    /// repository has no such remote, or it cannot be fetched within
    /// [`FETCH_TIMEOUT`] ([`Started::FellBack`]).
    Remote,
}

impl Start {
    /// The start `config` asks for.
    pub(crate) fn of(config: &Config) -> Self {
        match config.worktree_from_remote {
            true => Self::Remote,
            false => Self::Head,
        }
    }
}

/// What a worktree's branch started from, as the person who asked for it
/// is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Started {
    /// `HEAD`, as asked, or as [`Start::Remote`] in a repository with no
    /// [`REMOTE`].
    Head,
    /// The remote's default branch, as `origin/main`.
    Remote(String),
    /// `HEAD`, since the remote's default branch could not be fetched, for
    /// this reason.
    FellBack(String),
}

impl fmt::Display for Started {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Head => formatter.write_str("HEAD"),
            Self::Remote(branch) => formatter.write_str(branch),
            Self::FellBack(reason) => write!(
                formatter,
                "HEAD, since {REMOTE}'s default branch could not be fetched: {reason}"
            ),
        }
    }
}

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
    pub(crate) started: Started,
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
    /// It failed, with exit code `code` (`None` when a signal ended it),
    /// saying `stderr`, trimmed.
    Failed { code: Option<i32>, stderr: String },
}

/// What a worktree holds, as cleanup reads it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Holds {
    /// Uncommitted or untracked changes, which removing would lose.
    Changes,
    /// Nothing but `commit`, whose changes `base`, the `HEAD` of the
    /// checkout the worktree was added from or the remote's default branch,
    /// already has, merged, rebased, or squashed in: removing loses nothing.
    Merged { base: String, commit: String },
    /// Changes `base` does not have.
    Unmerged { base: String },
}

/// Adds a worktree under `root` for a session asked to work in
/// `workspace`, a canonical directory: of the repository `workspace` is in,
/// on a new branch from where `start` says. The worktree and its branch
/// share one name, the repository's directory name and a random suffix.
pub(crate) async fn add(
    root: &Path,
    workspace: &Path,
    start: Start,
) -> Result<Added, WorktreeError> {
    // In English, which the refusal below reads.
    let toplevel = match git(
        workspace,
        ["rev-parse", "--show-toplevel"],
        &[("LC_ALL", "C")],
    )
    .await?
    {
        Ran::Succeeded(stdout) => canonical(Path::new(&stdout))?,
        Ran::Failed { stderr, .. } if stderr.contains("not a git repository") => {
            return Err(WorktreeError::NotARepository(workspace.to_path_buf()));
        }
        Ran::Failed { stderr: detail, .. } => {
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
    // Without the Windows verbatim prefix a rho home can carry, which git
    // cannot create directories under.
    let path = canonical(root)?.join(&name);
    let head = || "HEAD".to_owned();
    let (started, commit) = match start {
        Start::Head => (Started::Head, head()),
        // Giving up kills the git commands still asking the remote.
        Start::Remote => match tokio::time::timeout(FETCH_TIMEOUT, fetch_tip(&toplevel)).await {
            Ok(Ok(Some(tip))) => (Started::Remote(tip.label), tip.commit),
            Ok(Ok(None)) => (Started::Head, head()),
            Ok(Err(error)) => (Started::FellBack(error.to_string()), head()),
            Err(_) => {
                let secs = FETCH_TIMEOUT.as_secs();
                let reason = format!("{REMOTE} did not answer within {secs} seconds");
                (Started::FellBack(reason), head())
            }
        },
    };
    // A commit, not the remote branch's name, so the new branch tracks
    // nothing: a push never lands on the remote's default branch.
    let args: [&OsStr; 6] = [
        "worktree".as_ref(),
        "add".as_ref(),
        "-b".as_ref(),
        branch.as_ref(),
        path.as_os_str(),
        commit.as_ref(),
    ];
    if let Ran::Failed { stderr: detail, .. } = git(&toplevel, args, &[]).await? {
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
        started,
    })
}

/// The commit the [`REMOTE`]'s default branch is at, as a repository last
/// fetched it.
struct RemoteTip {
    /// The branch, as `origin/main`.
    label: String,
    commit: String,
}

/// Fetches the [`REMOTE`]'s default branch into `repository`, recording it
/// as `origin/HEAD`, and returns its tip; `None` when the repository has no
/// such remote. The remote names its default each time, since git never
/// moves a recorded `origin/HEAD` when the remote's moves. A fetch that
/// fails is an error, never a stale commit.
async fn fetch_tip(repository: &Path) -> Result<Option<RemoteTip>, WorktreeError> {
    let remotes = succeeded(repository, "remote", ["remote"]).await?;
    if !remotes.lines().any(|remote| remote == REMOTE) {
        return Ok(None);
    }
    let listed = remote(
        repository,
        "ls-remote",
        ["ls-remote", "--symref", REMOTE, "HEAD"],
    )
    .await?;
    // `ref: refs/heads/main<TAB>HEAD`, absent when the remote's HEAD is
    // detached.
    let branch = listed
        .lines()
        .find_map(|line| {
            line.strip_prefix("ref: refs/heads/")?
                .strip_suffix("\tHEAD")
        })
        .ok_or_else(|| no_default(repository))?;
    fetch_branch(repository, branch).await?;
    let head = format!("refs/remotes/{REMOTE}/HEAD");
    let target = format!("refs/remotes/{REMOTE}/{branch}");
    let record = ["symbolic-ref", head.as_str(), target.as_str()];
    succeeded(repository, "symbolic-ref", record).await?;
    remote_tip(repository)
        .await?
        .ok_or_else(|| no_default(repository))
        .map(Some)
}

/// Fetches the [`REMOTE`]'s `branch` into its remote-tracking ref, leaving
/// `FETCH_HEAD` and submodules alone.
async fn fetch_branch(repository: &Path, branch: &str) -> Result<(), WorktreeError> {
    let refspec = format!("+refs/heads/{branch}:refs/remotes/{REMOTE}/{branch}");
    let fetch = [
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-write-fetch-head",
        "--no-recurse-submodules",
        REMOTE,
        refspec.as_str(),
    ];
    remote(repository, "fetch", fetch).await.map(drop)
}

fn no_default(repository: &Path) -> WorktreeError {
    WorktreeError::Git {
        action: "ls-remote".to_owned(),
        repository: repository.to_path_buf(),
        detail: format!("{REMOTE} names no default branch"),
    }
}

/// The [`REMOTE`]'s default branch, as `main`, that `repository` last
/// recorded, without asking the remote.
async fn remote_default(repository: &Path) -> Result<Option<String>, WorktreeError> {
    let reference = format!("refs/remotes/{REMOTE}/HEAD");
    let read = ["symbolic-ref", "--quiet", reference.as_str()];
    let prefix = format!("refs/remotes/{REMOTE}/");
    Ok(answered(repository, "symbolic-ref", read)
        .await?
        .and_then(|target| target.strip_prefix(&prefix).map(str::to_owned)))
}

/// The [`REMOTE`]'s default branch's tip as `repository` last fetched it,
/// without asking the remote; `None` when it records no default branch, or
/// never fetched the one it records.
async fn remote_tip(repository: &Path) -> Result<Option<RemoteTip>, WorktreeError> {
    let Some(branch) = remote_default(repository).await? else {
        return Ok(None);
    };
    let object = format!("refs/remotes/{REMOTE}/{branch}^{{commit}}");
    let verify = ["rev-parse", "--verify", "--quiet", object.as_str()];
    Ok(answered(repository, "rev-parse", verify)
        .await?
        .map(|commit| RemoteTip {
            label: format!("{REMOTE}/{branch}"),
            commit,
        }))
}

/// What `worktree` holds: whether removing it would lose uncommitted or
/// untracked changes, or committed changes both its origin's `HEAD` and the
/// remote's default branch, as last fetched, lack. It checks what the
/// worktree has checked out, which is its branch unless a session
/// switched: removing a worktree keeps every branch.
pub(crate) async fn inspect(worktree: &SessionWorktree) -> Result<Holds, WorktreeError> {
    // Every untracked file and submodule change, whatever the repository's
    // `status.showUntrackedFiles` or submodule settings hide: `git worktree
    // remove` follows those too, so it would not refuse what they hide.
    let status = succeeded(
        &worktree.path,
        "status",
        [
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    if !status.is_empty() {
        return Ok(Holds::Changes);
    }
    let head = succeeded(&worktree.path, "rev-parse", ["rev-parse", "HEAD"]).await?;
    let origin = &worktree.origin;
    // One commit for every check, though the origin's `HEAD` moves meanwhile.
    let base_commit = succeeded(origin, "rev-parse", ["rev-parse", "HEAD"]).await?;
    let base = match succeeded(origin, "rev-parse", ["rev-parse", "--abbrev-ref", "HEAD"]).await? {
        // A detached `HEAD`, which only its commit names.
        detached if detached == "HEAD" => {
            let short = ["rev-parse", "--short", base_commit.as_str()];
            succeeded(origin, "rev-parse", short).await?
        }
        branch => branch,
    };
    if has_changes_of(origin, &base_commit, &head).await? {
        return Ok(Holds::Merged { base, commit: head });
    }
    // A worktree started from the remote, or merged there, while the
    // checkout stayed behind.
    if let Some(tip) = remote_tip(origin).await? {
        if has_changes_of(origin, &tip.commit, &head).await? {
            let base = tip.label;
            return Ok(Holds::Merged { base, commit: head });
        }
    }
    Ok(Holds::Unmerged { base })
}

/// Whether `base` already has the changes of `commit`, both in the
/// repository at `dir`: it contains `commit`, as after a fast-forward or a
/// merge commit; it has a commit with the same patch as each of
/// `commit`'s own, as after a rebase merge or cherry-picks; or merging
/// `commit` into it would change nothing, as after a squash merge.
async fn has_changes_of(dir: &Path, base: &str, commit: &str) -> Result<bool, WorktreeError> {
    let ancestor = ["merge-base", "--is-ancestor", commit, base];
    if answered(dir, "merge-base", ancestor).await?.is_some() {
        return Ok(true);
    }
    // One line for each commit `base` lacks, merge commits left out: `-`
    // when `base` has one with the same patch, `+` when not. Unlike the
    // tree check below, it still matches once `base` edits those lines
    // again. A merge commit can add changes of its own, as a resolved
    // conflict does, which no patch here shows, so only a branch without
    // one is checked this way.
    let range = format!("{base}..{commit}");
    let merges = ["rev-list", "--merges", "--max-count=1", range.as_str()];
    if succeeded(dir, "rev-list", merges).await?.is_empty() {
        let cherry = succeeded(dir, "cherry", ["cherry", base, commit]).await?;
        if !cherry.is_empty() && cherry.lines().all(|line| line.starts_with('-')) {
            return Ok(true);
        }
    }
    // The tree merging `commit` into `base` makes, without touching any
    // checkout: `base`'s own when `base` has every change. Unlike patches,
    // it still matches when `base` changed lines next to them first, as
    // the squash commit of a branch behind its base does. Exit 1 is a
    // conflict.
    let merge = ["merge-tree", "--write-tree", base, commit];
    let tree = match answered(dir, "merge-tree", merge).await {
        Ok(Some(tree)) => tree,
        Ok(None) => return Ok(false),
        // As on a git before 2.38, which has no `--write-tree`, or for
        // unrelated histories: no squash merge can be told, so the worktree
        // reads as unmerged and stays, as before.
        Err(error) => {
            tracing::warn!(
                %error,
                "could not check for a squash merge (it needs git 2.38 or later); \
                 the worktree reads as unmerged"
            );
            return Ok(false);
        }
    };
    let base_tree = format!("{base}^{{tree}}");
    let base_tree = succeeded(dir, "rev-parse", ["rev-parse", base_tree.as_str()]).await?;
    Ok(tree.lines().next() == Some(base_tree.as_str()))
}

/// Removes `worktree` with `git worktree remove`, which refuses one with
/// uncommitted or untracked changes, and keeps its branch.
pub(crate) async fn remove(worktree: &SessionWorktree) -> Result<(), WorktreeError> {
    let args: [&OsStr; 3] = [
        "worktree".as_ref(),
        "remove".as_ref(),
        worktree.path.as_os_str(),
    ];
    succeeded(&worktree.origin, "worktree remove", args)
        .await
        .map(drop)
}

/// What [`restore`] checks out in the worktree it adds back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Checkout {
    /// Its branch, made again at the commit when someone deleted it.
    Branch,
    /// The commit alone, as when its branch is checked out elsewhere.
    Detached,
}

/// Adds `worktree` back where it was after [`remove`] removed it with
/// `commit` checked out, as `checkout` says, and makes `cwd`, the session's
/// directory in it, when git does not track it. When git no longer has
/// `commit`, as once a squash-merged branch is deleted and git collects
/// what only it held, it starts from the origin's `HEAD`, which [`inspect`]
/// found has its changes while the origin stays on the branch it checked,
/// unless only the remote's default branch had them: then the worktree
/// comes back without them, and they stay on the remote.
pub(crate) async fn restore(
    worktree: &SessionWorktree,
    commit: &str,
    cwd: &Path,
    checkout: Checkout,
) -> Result<(), WorktreeError> {
    let origin = &worktree.origin;
    let path = worktree.path.as_os_str();
    let branch: &OsStr = worktree.branch.as_ref();
    let object = format!("{commit}^{{commit}}");
    let exists = ["rev-parse", "--verify", "--quiet", object.as_str()];
    let head;
    let commit: &OsStr = match answered(origin, "rev-parse", exists).await? {
        Some(_) => commit.as_ref(),
        None => {
            head = succeeded(origin, "rev-parse", ["rev-parse", "HEAD"]).await?;
            tracing::warn!(
                branch = %worktree.branch,
                missing = %commit,
                %head,
                "git no longer has the commit a session worktree was removed at; \
                 adding it back at the origin's HEAD"
            );
            head.as_ref()
        }
    };
    let args: Vec<&OsStr> = match checkout {
        Checkout::Branch => {
            let reference = format!("refs/heads/{}", worktree.branch);
            let verify = ["rev-parse", "--verify", "--quiet", reference.as_str()];
            match answered(origin, "rev-parse", verify).await? {
                Some(_) => vec!["worktree".as_ref(), "add".as_ref(), path, branch],
                // Deleted since, as merged branches often are.
                None => vec![
                    "worktree".as_ref(),
                    "add".as_ref(),
                    "-b".as_ref(),
                    branch,
                    path,
                    commit,
                ],
            }
        }
        Checkout::Detached => vec![
            "worktree".as_ref(),
            "add".as_ref(),
            "--detach".as_ref(),
            path,
            commit,
        ],
    };
    succeeded(origin, "worktree add", args).await?;
    std::fs::create_dir_all(cwd).map_err(|error| WorktreeError::Io {
        path: cwd.to_path_buf(),
        error,
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

/// Who removes a session's worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Removal {
    /// The daemon, once it is clean and merged, keeping the branch.
    Daemon,
    /// A person: a foreground run's is no daemon's to remove.
    Manual,
}

/// What a run tells the person where its session works, as `rho run
/// --worktree` prints it.
pub(crate) fn hint(worktree: &SessionWorktree, removal: Removal) -> String {
    let removal = match removal {
        Removal::Daemon => "rho removes it once it is clean and merged, and keeps the branch",
        Removal::Manual => "rho never removes it",
    };
    format!(
        "the session works in worktree {} on branch {}; {removal}",
        worktree.path.display(),
        worktree.branch
    )
}

/// The output of `git args` in `dir`, or its failure as `action` there.
async fn succeeded(
    dir: &Path,
    action: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<String, WorktreeError> {
    match git(dir, args, &[]).await? {
        Ran::Succeeded(stdout) => Ok(stdout),
        Ran::Failed { stderr, .. } => Err(WorktreeError::Git {
            action: action.to_owned(),
            repository: dir.to_path_buf(),
            detail: stderr,
        }),
    }
}

/// The output of `git args` in `dir`, or `None` when git answers no with
/// exit 1, as `merge-base --is-ancestor` and `rev-parse --verify --quiet`
/// do; any other failure is one as `action` there.
async fn answered(
    dir: &Path,
    action: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<Option<String>, WorktreeError> {
    match git(dir, args, &[]).await? {
        Ran::Succeeded(stdout) => Ok(Some(stdout)),
        Ran::Failed { code: Some(1), .. } => Ok(None),
        Ran::Failed { stderr, .. } => Err(WorktreeError::Git {
            action: action.to_owned(),
            repository: dir.to_path_buf(),
            detail: stderr,
        }),
    }
}

/// [`succeeded`] for a git command that asks a remote, so may wait on the
/// network or on a person. It runs in a process tree of its own, which on
/// Unix has no terminal, so ssh cannot stop it asking for a passphrase or a
/// host key; dropping the future kills the whole tree, ssh included.
async fn remote(
    dir: &Path,
    action: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<String, WorktreeError> {
    let mut command = command(dir, args, &[]);
    SupervisedTree::prepare(&mut command);
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(WorktreeError::NoGit)?;
    let _tree = SupervisedTree::attach(&child).map_err(WorktreeError::NoGit)?;
    let output = child
        .wait_with_output()
        .await
        .map_err(WorktreeError::NoGit)?;
    match ran(&output) {
        Ran::Succeeded(stdout) => Ok(stdout),
        Ran::Failed { stderr, .. } => Err(WorktreeError::Git {
            action: action.to_owned(),
            repository: dir.to_path_buf(),
            detail: stderr,
        }),
    }
}

/// Runs `git args` in `dir` with `env` set.
async fn git(
    dir: &Path,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    env: &[(&str, &str)],
) -> Result<Ran, WorktreeError> {
    let output = command(dir, args, env)
        .output()
        .await
        .map_err(WorktreeError::NoGit)?;
    Ok(ran(&output))
}

/// The command for `git args` in `dir` with `env` set, outside any
/// repository the caller's environment names.
fn command(
    dir: &Path,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    env: &[(&str, &str)],
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    for name in crate::git::LOCATION_ENV {
        command.env_remove(name);
    }
    // Its stdin is no terminal: git fails rather than asking for a
    // username or password.
    command.env("GIT_TERMINAL_PROMPT", "0");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null());
    command
}

fn ran(output: &std::process::Output) -> Ran {
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    if output.status.success() {
        Ran::Succeeded(text(&output.stdout))
    } else {
        Ran::Failed {
            code: output.status.code(),
            stderr: text(&output.stderr),
        }
    }
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
