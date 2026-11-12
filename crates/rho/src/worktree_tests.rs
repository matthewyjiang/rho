use std::{path::Path, process::Command};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{add, WorktreeError};

/// Runs `git args` in `dir`, isolated from the caller's repository and
/// config, and returns its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for name in crate::git::LOCATION_ENV {
        command.env_remove(name);
    }
    let output = command
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=rho",
            "-c",
            "user.email=rho@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

// Covers: a session asked to work in a repository's subdirectory gets a
// worktree under the root, on a new branch from HEAD that the worktree has
// checked out, and works at the same subdirectory in it; the repository's
// own checkout keeps its branch and stays clean.
// Owner: session worktrees.
#[tokio::test]
async fn a_worktree_branches_from_head_and_keeps_the_workspace_place() {
    let home = TempDir::new().unwrap();
    let repository = home.path().join("my repo");
    std::fs::create_dir_all(repository.join("src")).unwrap();
    git(&repository, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(repository.join("src/lib.txt"), "base").unwrap();
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "--quiet", "-m", "base"]);
    let workspace = crate::paths::canonicalize(&repository.join("src")).unwrap();
    let root = home.path().join("rho-home/worktrees");

    let added = add(&root, &workspace).await.unwrap();

    let path = &added.worktree.path;
    assert_eq!(
        (
            added.worktree.branch.starts_with("rho/my-repo-"),
            path.parent() == Some(crate::paths::canonicalize(&root).unwrap().as_path()),
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            added.cwd.clone(),
            added.worktree.origin.clone(),
            git(path, &["branch", "--show-current"]),
            std::fs::read_to_string(added.cwd.join("lib.txt")).unwrap(),
            git(&repository, &["branch", "--show-current"]),
            git(&repository, &["status", "--porcelain"]),
        ),
        (
            true,
            true,
            added
                .worktree
                .branch
                .strip_prefix("rho/")
                .map(str::to_owned),
            path.join("src"),
            workspace,
            added.worktree.branch.clone(),
            "base".to_owned(),
            "main".to_owned(),
            String::new(),
        )
    );
}

// Covers: a workspace in no git repository gets no worktree, and the error
// says why.
// Owner: session worktrees.
#[tokio::test]
async fn a_workspace_outside_git_is_refused() {
    let home = TempDir::new().unwrap();
    let workspace = crate::paths::canonicalize(home.path()).unwrap();

    let refused = add(&home.path().join("worktrees"), &workspace).await;

    assert!(
        matches!(&refused, Err(WorktreeError::NotARepository(path)) if path == &workspace),
        "{refused:?}"
    );
}
