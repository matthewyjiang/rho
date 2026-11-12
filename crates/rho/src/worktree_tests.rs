use std::{path::Path, process::Command};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{add, inspect, remove, restore, Checkout, Holds, WorktreeError};

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

/// A repository in `home` whose one commit on `main` has `shared.txt`.
fn repository(home: &Path) -> std::path::PathBuf {
    let repository = home.join("repo");
    std::fs::create_dir_all(&repository).unwrap();
    git(&repository, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(repository.join("shared.txt"), "base").unwrap();
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "--quiet", "-m", "base"]);
    crate::paths::canonicalize(&repository).unwrap()
}

// Covers: cleanup reads a worktree with uncommitted, staged, or untracked
// changes as holding changes, also when the repository's config hides
// untracked files from `git status` and `git worktree remove`, one whose
// commit its origin's HEAD lacks as unmerged, and one with nothing new, or
// whose commit was merged, as merged at its commit, naming the origin's
// branch, or the origin's commit when detached.
// Owner: session worktrees (git plumbing under the daemon's cleanup).
#[tokio::test]
async fn inspect_reads_what_removing_a_worktree_would_lose() {
    let home = TempDir::new().unwrap();
    let origin = repository(home.path());
    let root = home.path().join("worktrees");
    let commit = |path: &Path| {
        std::fs::write(path.join("new.txt"), "work").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "--quiet", "-m", "work"]);
    };
    let mut found = Vec::new();
    let mut expected = Vec::new();
    // What the check finds, given the worktree's commit.
    type Finds = fn(String) -> Holds;
    let main: Finds = |commit| Holds::Merged {
        base: "main".to_owned(),
        commit,
    };
    let changes: Finds = |_| Holds::Changes;
    // What a case does in its new worktree before the check.
    type Arrange<'a> = &'a dyn Fn(&Path);
    let cases: [(&str, Arrange, Finds); 7] = [
        ("nothing new", &|_| {}, main),
        (
            "untracked",
            &|path| std::fs::write(path.join("new.txt"), "work").unwrap(),
            changes,
        ),
        (
            "modified",
            &|path| std::fs::write(path.join("shared.txt"), "edit").unwrap(),
            changes,
        ),
        (
            "staged",
            &|path| {
                std::fs::write(path.join("shared.txt"), "edit").unwrap();
                git(path, &["add", "."]);
            },
            changes,
        ),
        ("unmerged commit", &commit, |_| Holds::Unmerged {
            base: "main".to_owned(),
        }),
        (
            "merged commit",
            &|path| {
                commit(path);
                let branch = git(path, &["branch", "--show-current"]);
                git(&origin, &["merge", "--quiet", "--ff-only", branch.as_str()]);
            },
            main,
        ),
        // Last: the setting is the whole repository's.
        (
            "untracked, hidden by config",
            &|path| {
                git(path, &["config", "status.showUntrackedFiles", "no"]);
                std::fs::write(path.join("hidden.txt"), "work").unwrap();
            },
            changes,
        ),
    ];
    for (case, arrange, holds) in cases {
        let added = add(&root, &origin).await.unwrap();
        arrange(&added.worktree.path);
        found.push((case, inspect(&added.worktree).await.unwrap()));
        expected.push((
            case,
            holds(git(&added.worktree.path, &["rev-parse", "HEAD"])),
        ));
    }
    git(&origin, &["checkout", "--quiet", "--detach"]);
    let detached = add(&root, &origin).await.unwrap();
    found.push((
        "detached origin",
        inspect(&detached.worktree).await.unwrap(),
    ));
    expected.push((
        "detached origin",
        Holds::Merged {
            base: git(&origin, &["rev-parse", "--short", "HEAD"]),
            commit: git(&origin, &["rev-parse", "HEAD"]),
        },
    ));

    assert_eq!(found, expected);
}

// Covers: removing a worktree keeps its branch, and restoring it at the
// commit it was removed at checks the branch out at the same path again,
// makes the branch again at that commit, not the origin's HEAD, when
// someone deleted it, and checks the commit out alone when asked to, as a
// read does when the branch is checked out elsewhere. The session's
// directory in it is made even when git tracks nothing there.
// Owner: session worktrees (git plumbing under the daemon's cleanup).
#[tokio::test]
async fn a_removed_worktree_comes_back_at_the_commit_it_was_removed_at() {
    let home = TempDir::new().unwrap();
    let origin = repository(home.path());
    let root = home.path().join("worktrees");
    let removed_at = git(&origin, &["rev-parse", "HEAD"]);
    let mut found = Vec::new();
    let mut expected = Vec::new();
    for (case, delete_branch, checkout) in [
        ("branch kept", false, Checkout::Branch),
        ("branch deleted", true, Checkout::Branch),
        ("detached", false, Checkout::Detached),
    ] {
        let added = add(&root, &origin).await.unwrap();
        let worktree = &added.worktree;
        remove(worktree).await.unwrap();
        let branch_kept = !git(&origin, &["branch", "--list", worktree.branch.as_str()]).is_empty();
        if delete_branch {
            git(
                &origin,
                &["branch", "--quiet", "-D", worktree.branch.as_str()],
            );
        }
        // The origin moves on, so a branch made from its HEAD would differ.
        std::fs::write(origin.join(format!("{case}.txt")), case).unwrap();
        git(&origin, &["add", "."]);
        git(&origin, &["commit", "--quiet", "-m", case]);
        let cwd = worktree.path.join("untracked/dir");

        restore(worktree, &removed_at, &cwd, checkout)
            .await
            .unwrap();

        found.push((
            case,
            branch_kept,
            git(&worktree.path, &["branch", "--show-current"]),
            git(&worktree.path, &["rev-parse", "HEAD"]),
            cwd.is_dir(),
        ));
        let branch = match checkout {
            Checkout::Branch => worktree.branch.clone(),
            Checkout::Detached => String::new(),
        };
        expected.push((case, true, branch, removed_at.clone(), true));
    }

    assert_eq!(found, expected);
}
