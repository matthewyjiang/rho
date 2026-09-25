//! `/diff` popup: changed files grouped by git area, one file's patch at a time.

use std::{fs, path::Path, process::Command};

use anyhow::{ensure, Context, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

pub(super) const DIFF_VIEWER_SCENARIO: Scenario = Scenario::new(
    "diff_viewer",
    "Browse staged, unstaged, and untracked files in the /diff popup",
    PtySize {
        rows: 30,
        cols: 120,
    },
    DIFF_VIEWER_STEPS,
    false,
)
.with_setup(setup_git_worktree);

/// One file staged then edited again (so it is listed under both Staged and
/// Unstaged) and one untracked file whose patch loads only when selected.
fn setup_git_worktree(home: &IsolatedHome) -> Result<()> {
    let repo = &home.workspace;
    fs::write(repo.join("app.py"), "print('one')\n")?;
    git(repo, &["init", "-q", "-b", "main"])?;
    git(repo, &["add", "app.py"])?;
    git(repo, &["commit", "-q", "-m", "init"])?;
    fs::write(repo.join("app.py"), "print('index-copy')\n")?;
    git(repo, &["add", "app.py"])?;
    fs::write(repo.join("app.py"), "print('worktree-copy')\n")?;
    fs::write(repo.join("notes.md"), "untracked-body\n")?;
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=rho",
            "-c",
            "user.email=rho@example.invalid",
        ])
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .with_context(|| format!("run git {args:?}"))?;
    ensure!(status.success(), "git {args:?} failed");
    Ok(())
}

const DIFF_VIEWER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_on_first_staged_file"),
    Step::SubmitText("/diff"),
    Step::WaitText {
        text: "Unstaged",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "index-copy",
        timeout: SETTLE,
    },
    Step::Phase("tab_to_unstaged_copy"),
    Step::Key(Key::Tab),
    Step::WaitText {
        text: "worktree-copy",
        timeout: SETTLE,
    },
    Step::Phase("filter_to_untracked_file"),
    Step::TypeText("notes"),
    Step::WaitText {
        text: "untracked-body",
        timeout: SETTLE,
    },
    Step::Phase("close"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Untracked",
        timeout: SETTLE,
    },
    Step::Custom(commit_everything),
    Step::Phase("clean_worktree_opens_empty_popup"),
    Step::SubmitText("/diff"),
    // The popup title, not the status line, which also says "clean".
    Step::WaitText {
        text: "Diff · main · clean",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];

fn commit_everything(harness: &mut PtyHarness) -> Result<()> {
    let repo = harness
        .working_directory()
        .context("pty harness has no working directory")?
        .to_path_buf();
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "all"])
}
