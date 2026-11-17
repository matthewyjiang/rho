//! Telling a squash merge `base` moved on from: once `base` edits the lines
//! a squashed branch changed, merging the branch into it conflicts, and its
//! commits match no single patch there, so only the squash commit itself
//! shows the branch's changes landed.

use std::{path::Path, process::Stdio};

use tokio::io::AsyncWriteExt as _;

use super::{answered, command, succeeded, WorktreeError};

/// What every patch here is printed with, whatever the person's config
/// sets: color, external diffs, text conversion, rename detection, or a
/// diff relative to a subdirectory would change the patch IDs, or which
/// changes they cover. Pathspecs are taken literally.
const PLAIN: [&str; 6] = [
    "--literal-pathspecs",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-renames",
    "--no-relative",
];

/// Whether a commit on `base` since it and `commit` parted makes exactly
/// the changes `commit` has since then, in the files it changed, as the
/// squash commit a forge makes of a pull request does, both in the
/// repository at `dir`. Patch IDs ignore line numbers, so the squash commit
/// matches however far `base` moved on after.
pub(super) async fn squashed_into(
    dir: &Path,
    base: &str,
    commit: &str,
) -> Result<bool, WorktreeError> {
    let Some(fork) = answered(dir, "merge-base", ["merge-base", base, commit]).await? else {
        return Ok(false);
    };
    let Some(wanted) = patch_ids(dir, "diff", &[fork.as_str(), commit], None)
        .await?
        .into_iter()
        .next()
    else {
        // No net change, which the tree check before this one reads.
        return Ok(false);
    };
    // Only the commits touching those files: on rho's history, 500 commits
    // took 1.4 s to print whole and 0.2 s limited to a branch's 20 files.
    // A commit touching more matches when, in those files, it makes the
    // branch's changes, which `base` then has. The files go on stdin, which
    // no command-line limit bounds.
    let names = ["diff", "--name-only", "--no-renames", "--no-relative"];
    let names = succeeded(dir, "diff", [&names[..], &[fork.as_str(), commit]].concat()).await?;
    let mut stdin = format!("{fork}..{base}\n--\n");
    for name in names.lines() {
        stdin.push_str(name);
        stdin.push('\n');
    }
    let log = ["--stdin", "--no-merges", "--patch", "--format=commit %H"];
    let found = patch_ids(dir, "log", &log, Some(stdin))
        .await?
        .contains(&wanted);
    if found {
        tracing::debug!(%fork, %commit, %base, "found the squash commit of a worktree's branch");
    }
    Ok(found)
}

/// The stable patch ID of each patch `git <subcommand> PLAIN args`
/// prints in `dir`, given `stdin`. The patches stream from that git
/// process into `git patch-id`, so a long history never sits in memory.
async fn patch_ids(
    dir: &Path,
    subcommand: &str,
    args: &[&str],
    stdin: Option<String>,
) -> Result<Vec<String>, WorktreeError> {
    let failed = |action: &str, output: &std::process::Output| WorktreeError::Git {
        action: action.to_owned(),
        repository: dir.to_path_buf(),
        detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    };
    let (reader, writer) = std::io::pipe().map_err(WorktreeError::NoGit)?;
    let [literal, rest @ ..] = PLAIN;
    let full = [&[literal, subcommand][..], &rest, args].concat();
    // Each builder drops once its child spawns, closing its copy of its
    // pipe end, so neither side waits on a reader or writer that is gone.
    let mut patches = command(dir, full, &[])
        .stdin(match stdin {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(writer)
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(WorktreeError::NoGit)?;
    let ids = command(dir, ["patch-id", "--stable"], &[])
        .stdin(reader)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(WorktreeError::NoGit)?;
    if let (Some(input), Some(mut pipe)) = (stdin, patches.stdin.take()) {
        // `--stdin` reads it all before printing, so this never waits on
        // the patches. Closed after, which ends git's input; a short write
        // shows as git's own failure below.
        let _ = pipe.write_all(input.as_bytes()).await;
    }
    let (ids, patched) = tokio::join!(ids.wait_with_output(), patches.wait_with_output());
    let (ids, patched) = (
        ids.map_err(WorktreeError::NoGit)?,
        patched.map_err(WorktreeError::NoGit)?,
    );
    // First: when `patch-id` dies, the patches fail on the closed pipe.
    if !ids.status.success() {
        return Err(failed("patch-id", &ids));
    }
    if !patched.status.success() {
        return Err(failed(subcommand, &patched));
    }
    // `<patch id> <commit>`, one line per patch.
    Ok(String::from_utf8_lossy(&ids.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect())
}
