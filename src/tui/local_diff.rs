use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct WorktreeDiff {
    pub(super) lines: Vec<String>,
    pub(super) has_changes: bool,
}

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
        for path in untracked {
            lines.extend(untracked_patch(cwd, &path)?);
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
    use std::fs;

    use super::*;

    #[test]
    fn reports_status_and_patch_for_modified_worktree() {
        let dir = std::env::temp_dir().join(format!("rho-diff-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet"]).unwrap();
        git(&dir, &["config", "user.email", "rho@example.test"]).unwrap();
        git(&dir, &["config", "user.name", "rho test"]).unwrap();
        fs::write(dir.join("file.txt"), "old\n").unwrap();
        git(&dir, &["add", "file.txt"]).unwrap();
        git(&dir, &["commit", "--quiet", "-m", "initial"]).unwrap();
        fs::write(dir.join("file.txt"), "new \n").unwrap();

        let diff = collect(&dir).unwrap();

        assert!(diff.has_changes);
        assert!(diff.lines.iter().any(|line| line == " M file.txt"));
        assert!(diff.lines.iter().any(|line| line == "-old"));
        assert!(diff.lines.iter().any(|line| line == "+new "));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn includes_untracked_file_patch() {
        let dir = std::env::temp_dir().join(format!("rho-diff-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet"]).unwrap();
        fs::write(dir.join("new.txt"), "untracked contents\n").unwrap();

        let diff = collect(&dir).unwrap();

        assert!(diff.lines.iter().any(|line| line == "?? new.txt"));
        assert!(diff.lines.iter().any(|line| line == "+untracked contents"));
        fs::remove_dir_all(dir).unwrap();
    }
}
