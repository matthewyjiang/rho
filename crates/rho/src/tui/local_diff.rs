//! Git data behind the `/diff` popup.
//!
//! Loading happens in two phases so `/diff` opens quickly in large worktrees.
//! [`collect_status`] lists changed files with line counts: one `git status`
//! plus two `git diff --numstat` runs, and no patch text. [`file_patch`] then
//! loads one file's patch when the popup selects it.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{anyhow, bail, Context};

/// Git area a changed file belongs to. A path staged and then edited again
/// appears once in each of Staged and Unstaged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiffSection {
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileChange {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChangedFile {
    pub(super) section: DiffSection,
    pub(super) change: FileChange,
    /// Repo-root-relative path (the new path for renames/copies).
    pub(super) path: String,
    /// Rename/copy source, repo-root-relative.
    pub(super) orig_path: Option<String>,
    /// `(added, removed)` line counts. `None` for binary files, for files
    /// git reported no count for, and for untracked files, which are not
    /// diffed until selected.
    pub(super) stats: Option<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WorktreeStatus {
    /// Absolute repo top-level; `file_patch` runs git here so porcelain paths resolve.
    pub(super) repo_root: PathBuf,
    /// Current branch, `None` when HEAD is detached.
    pub(super) branch: Option<String>,
    /// Ordered Staged, then Unstaged, then Untracked; git's path order within each.
    pub(super) files: Vec<ChangedFile>,
}

/// Line counts per path, as parsed from `git diff --numstat -z`.
type NumstatMap = HashMap<String, Option<(u64, u64)>>;

/// Cheap listing run when `/diff` opens: status plus per-file line counts,
/// no patch text.
///
/// `cwd` may be anywhere inside the worktree; the listing covers the whole repo.
pub(super) fn collect_status(cwd: &Path) -> anyhow::Result<WorktreeStatus> {
    let toplevel = checked_stdout(run(git(cwd).args(["rev-parse", "--show-toplevel"]))?)?;
    let repo_root = path_from_git_bytes(toplevel.strip_suffix(b"\n").unwrap_or(&toplevel[..]));

    // `--renames` and `-M` force rename detection on both sides regardless of
    // user config, so renamed status entries find their numstat counts.
    let status = checked_stdout(run(git(&repo_root).args([
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=all",
        "--renames",
    ]))?)?;
    let (branch, mut files) = parse_status(&status)?;

    // `--cached` compares against the empty tree when HEAD is unborn, so this
    // also works before the first commit.
    let staged = parse_numstat(&checked_stdout(run(git(&repo_root).args([
        "diff",
        "--cached",
        "--numstat",
        "-z",
        "-M",
    ]))?)?)?;
    let unstaged = parse_numstat(&checked_stdout(run(git(&repo_root).args([
        "diff",
        "--numstat",
        "-z",
        "-M",
    ]))?)?)?;
    for file in &mut files {
        let counts = match file.section {
            DiffSection::Staged => &staged,
            DiffSection::Unstaged => &unstaged,
            DiffSection::Untracked => continue,
        };
        file.stats = counts.get(&file.path).copied().flatten();
    }

    Ok(WorktreeStatus {
        repo_root,
        branch,
        files,
    })
}

/// Unified patch text for one file. Blocking; the caller runs it off the event loop.
///
/// Unmerged files diff the worktree against "ours" (`--ours`) so the patch
/// stays a plain two-way diff with conflict markers as added lines, rather
/// than git's default combined `diff --cc` format.
pub(super) fn file_patch(repo_root: &Path, file: &ChangedFile) -> anyhow::Result<String> {
    let mut command = git(repo_root);
    // Explicit prefixes override `diff.noprefix`/`diff.mnemonicPrefix` so
    // headers always read `a/…` and `b/…`.
    command.args([
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]);
    match file.section {
        DiffSection::Staged => {
            command.args(["--cached", "-M"]);
        }
        DiffSection::Unstaged if file.change == FileChange::Unmerged => {
            command.arg("--ours");
        }
        DiffSection::Unstaged => {
            command.arg("-M");
        }
        DiffSection::Untracked => return untracked_patch(command, &file.path),
    }
    // Passing the rename source too lets git pair both sides into one rename.
    command.arg("--").arg(&file.path).args(&file.orig_path);
    command_output(run(&mut command)?)
}

/// `git` at `dir` with settings that keep output stable for parsing and
/// display: literal pathspecs (so `*` and `[` in names match only themselves),
/// no optional index refresh racing other git processes, and unquoted UTF-8
/// paths in patch headers.
fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(["--no-optional-locks", "-c", "core.quotePath=false"]);
    command
}

fn run(command: &mut Command) -> anyhow::Result<Output> {
    command
        .output()
        .map_err(|error| anyhow!("could not run git: {error}"))
}

/// Diffs an untracked file against `/dev/null` with `git diff --no-index`.
fn untracked_patch(mut command: Command, path: &str) -> anyhow::Result<String> {
    // With `--untracked-files=all`, status reports a directory only for a
    // nested repository, which `--no-index` cannot diff against a file.
    if path.ends_with('/') {
        bail!("{path} is a nested git repository");
    }
    let output = run(command.args(["--no-index", "--", "/dev/null", path]))?;
    // `--no-index` exits 1 when the inputs differ, which a file always does
    // against /dev/null. Exit 1 with no patch means git failed instead, for
    // example because the file was removed after the listing.
    if output.status.code() == Some(1) && !output.stdout.is_empty() {
        return Ok(trimmed_text(&output.stdout));
    }
    command_output(output)
}

/// Parses `git status --porcelain=v2 -z --branch` into the branch name and
/// the changed files in display order, with `stats` left `None`.
fn parse_status(bytes: &[u8]) -> anyhow::Result<(Option<String>, Vec<ChangedFile>)> {
    let mut branch = None;
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    let mut records = bytes.split(|byte| *byte == 0).map(String::from_utf8_lossy);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let Some((kind, rest)) = record.split_once(' ') else {
            bail!("unexpected git status record: {record:?}");
        };
        match kind {
            "#" => {
                if let Some(head) = rest.strip_prefix("branch.head ") {
                    branch = (head != "(detached)").then(|| head.to_string());
                }
            }
            // `1 XY sub mH mI mW hH hI path`
            "1" => {
                let (xy, path) = xy_and_path(rest, 8, &record)?;
                push_tracked(&mut staged, &mut unstaged, xy, path, None)?;
            }
            // `2 XY sub mH mI mW hH hI Xscore path`, then the source path as
            // the next NUL-separated record.
            "2" => {
                let (xy, path) = xy_and_path(rest, 9, &record)?;
                let orig = records
                    .next()
                    .filter(|orig| !orig.is_empty())
                    .with_context(|| format!("git status rename has no source: {record:?}"))?;
                push_tracked(&mut staged, &mut unstaged, xy, path, Some(&*orig))?;
            }
            // `u XY sub m1 m2 m3 mW h1 h2 h3 path`
            "u" => {
                let (_, path) = xy_and_path(rest, 10, &record)?;
                unstaged.push(ChangedFile {
                    section: DiffSection::Unstaged,
                    change: FileChange::Unmerged,
                    path: path.to_string(),
                    orig_path: None,
                    stats: None,
                });
            }
            "?" => untracked.push(ChangedFile {
                section: DiffSection::Untracked,
                change: FileChange::Untracked,
                path: rest.to_string(),
                orig_path: None,
                stats: None,
            }),
            // Ignored files are not listed without `--ignored`, but skip them
            // defensively rather than failing the whole listing.
            "!" => {}
            _ => bail!("unexpected git status record: {record:?}"),
        }
    }
    staged.extend(unstaged);
    staged.extend(untracked);
    Ok((branch, staged))
}

/// Splits the fields after a status entry's kind into its `XY` code and path.
/// `field_count` includes the path, which is last and may contain spaces.
fn xy_and_path<'a>(
    rest: &'a str,
    field_count: usize,
    record: &str,
) -> anyhow::Result<(&'a str, &'a str)> {
    let fields: Vec<&str> = rest.splitn(field_count, ' ').collect();
    match (fields.first(), fields.get(field_count - 1)) {
        (Some(&xy), Some(&path)) if fields.len() == field_count && !path.is_empty() => {
            Ok((xy, path))
        }
        _ => bail!("unexpected git status record: {record:?}"),
    }
}

/// Adds a Staged entry when `X` is set and an Unstaged entry when `Y` is set.
/// Only a side that is itself a rename or copy carries `orig`.
fn push_tracked(
    staged: &mut Vec<ChangedFile>,
    unstaged: &mut Vec<ChangedFile>,
    xy: &str,
    path: &str,
    orig: Option<&str>,
) -> anyhow::Result<()> {
    let &[x, y] = xy.as_bytes() else {
        bail!("unexpected git status code: {xy:?}");
    };
    for (letter, section, files) in [
        (x, DiffSection::Staged, staged),
        (y, DiffSection::Unstaged, unstaged),
    ] {
        let Some(change) = file_change(letter)? else {
            continue;
        };
        let orig_path = orig
            .filter(|_| matches!(change, FileChange::Renamed | FileChange::Copied))
            .map(str::to_string);
        files.push(ChangedFile {
            section,
            change,
            path: path.to_string(),
            orig_path,
            stats: None,
        });
    }
    Ok(())
}

/// Maps one porcelain status letter; `.` means that side is unchanged.
fn file_change(letter: u8) -> anyhow::Result<Option<FileChange>> {
    let change = match letter {
        b'.' => return Ok(None),
        b'M' => FileChange::Modified,
        b'A' => FileChange::Added,
        b'D' => FileChange::Deleted,
        b'R' => FileChange::Renamed,
        b'C' => FileChange::Copied,
        b'T' => FileChange::TypeChanged,
        _ => bail!("unexpected git status code: {:?}", char::from(letter)),
    };
    Ok(Some(change))
}

/// Parses `git diff --numstat -z` into `(added, removed)` per path, keyed by
/// the new path for renames. Binary files map to `None`. When a path repeats,
/// as unmerged paths do, the later record (the comparison against "ours") wins.
fn parse_numstat(bytes: &[u8]) -> anyhow::Result<NumstatMap> {
    let mut stats = HashMap::new();
    let mut records = bytes.split(|byte| *byte == 0).map(String::from_utf8_lossy);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let malformed = || anyhow!("unexpected git numstat record: {record:?}");
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(malformed());
        };
        let counts = match (added, removed) {
            ("-", "-") => None,
            _ => Some((
                added.parse::<u64>().map_err(|_| malformed())?,
                removed.parse::<u64>().map_err(|_| malformed())?,
            )),
        };
        let path = if path.is_empty() {
            // Renames leave the path field empty and follow with two records:
            // the source path, then the destination path.
            records.next().ok_or_else(malformed)?;
            records
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(malformed)?
                .into_owned()
        } else {
            path.to_string()
        };
        stats.insert(path, counts);
    }
    Ok(stats)
}

/// Stdout as text on success; otherwise the first stderr line as the error.
fn command_output(output: Output) -> anyhow::Result<String> {
    checked_stdout(output).map(|stdout| trimmed_text(&stdout))
}

/// Raw stdout on success; otherwise the first stderr line as the error.
fn checked_stdout(output: Output) -> anyhow::Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = String::from_utf8_lossy(&output.stderr);
    let first_line = message.trim().lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        bail!("git command failed");
    }
    bail!("{first_line}")
}

fn trimmed_text(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

#[cfg(unix)]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    PathBuf::from(OsStr::from_bytes(path))
}

#[cfg(not(unix))]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(path).into_owned())
}

#[cfg(test)]
#[path = "local_diff_tests.rs"]
mod tests;
