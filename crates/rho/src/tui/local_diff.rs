use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use rho_tools::tool_card::{DiffRow, DiffRowKind};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct WorktreeDiff {
    pub(super) lines: Vec<String>,
    pub(super) has_changes: bool,
}

impl WorktreeDiff {
    /// Patch text as diff rows so `/diff` colors changes like a tool card.
    ///
    /// Git output here mixes status, section titles and several patches, so
    /// rows carry no line numbers and the body renders without a gutter.
    pub(super) fn rows(&self) -> Vec<DiffRow> {
        self.lines
            .iter()
            .map(|line| match line.as_bytes().first() {
                Some(b'+') if !line.starts_with("+++") => {
                    DiffRow::new(DiffRowKind::Added, None, &line[1..])
                }
                Some(b'-') if !line.starts_with("---") => {
                    DiffRow::new(DiffRowKind::Removed, None, &line[1..])
                }
                Some(_) | None => DiffRow::new(DiffRowKind::Context, None, line.clone()),
            })
            .collect()
    }
}

/// Bound per-file `git diff --no-index` spawns so `/diff` stays responsive in
/// workspaces where ignore rules leave thousands of untracked files behind.
const MAX_UNTRACKED_PATCHES: usize = 50;

pub(super) fn collect(cwd: &Path) -> anyhow::Result<WorktreeDiff> {
    let status = git(cwd, &["status", "--short", "--branch"])?;
    let unstaged = git(cwd, &["diff", "--no-ext-diff", "--"])?;
    let staged = git(cwd, &["diff", "--cached", "--no-ext-diff", "--"])?;
    let untracked = untracked_files(cwd)?;
    let has_changes = status
        .lines()
        .any(|line| !line.starts_with("##") && !line.trim().is_empty());

    let mut lines = section("status", &status);
    if !staged.is_empty() {
        lines.push(String::new());
        lines.extend(section("staged changes", &staged));
    }
    if !unstaged.is_empty() {
        lines.push(String::new());
        lines.extend(section("unstaged changes", &unstaged));
    }
    if !untracked.is_empty() {
        lines.push(String::new());
        lines.push("untracked changes:".into());
        let shown = untracked.len().min(MAX_UNTRACKED_PATCHES);
        for path in &untracked[..shown] {
            lines.extend(untracked_patch(cwd, path)?);
        }
        let remaining = untracked.len() - shown;
        if remaining > 0 {
            lines.push(format!("... {remaining} more untracked files not diffed"));
        }
    }
    if !has_changes {
        lines.push(String::new());
        lines.push("worktree clean".into());
    }

    Ok(WorktreeDiff { lines, has_changes })
}

fn git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    command_output(Command::new("git").current_dir(cwd).args(args).output()?)
}

fn command_output(output: Output) -> anyhow::Result<String> {
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!(if message.is_empty() {
            "git command failed".to_string()
        } else {
            message
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

fn untracked_files(cwd: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;
    if !output.status.success() {
        command_output(output)?;
        unreachable!();
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_git_bytes)
        .collect())
}

#[cfg(unix)]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(path))
}

#[cfg(not(unix))]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(path).into_owned())
}

fn untracked_patch(cwd: &Path, path: &Path) -> anyhow::Result<Vec<String>> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args([
            OsStr::new("diff"),
            OsStr::new("--no-index"),
            OsStr::new("--"),
            OsStr::new("/dev/null"),
            path.as_os_str(),
        ])
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return command_output(output).map(|text| text.lines().map(str::to_string).collect());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .lines()
        .map(str::to_string)
        .collect())
}

fn section(title: &str, content: &str) -> Vec<String> {
    let mut lines = vec![format!("{title}:")];
    lines.extend(content.lines().map(str::to_string));
    lines
}

#[cfg(test)]
mod tests {

    use rho_tools::tool_card::DiffRowKind;

    use super::*;

    #[test]
    fn rows_classifies_added_removed_and_keeps_headers_as_context() {
        let diff = WorktreeDiff {
            lines: vec![
                "--- a/file.txt".into(),
                "+++ b/file.txt".into(),
                " context".into(),
                "-old".into(),
                "+new".into(),
            ],
            has_changes: true,
        };

        let rows = diff.rows();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].kind, DiffRowKind::Context);
        assert_eq!(rows[0].text, "--- a/file.txt");
        assert_eq!(rows[1].kind, DiffRowKind::Context);
        assert_eq!(rows[1].text, "+++ b/file.txt");
        assert_eq!(rows[2].kind, DiffRowKind::Context);
        assert_eq!(rows[2].text, " context");
        assert_eq!(rows[3].kind, DiffRowKind::Removed);
        assert_eq!(rows[3].text, "old");
        assert_eq!(rows[4].kind, DiffRowKind::Added);
        assert_eq!(rows[4].text, "new");
    }
}
