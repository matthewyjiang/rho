use std::{path::Path, process::Command};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{add, inspect, remove, restore, Checkout, Holds, Start, Started, WorktreeError};

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

    let added = add(&root, &workspace, Start::Head).await.unwrap();

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

    let refused = add(&home.path().join("worktrees"), &workspace, Start::Head).await;

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
        let added = add(&root, &origin, Start::Head).await.unwrap();
        arrange(&added.worktree.path);
        found.push((case, inspect(&added.worktree).await.unwrap()));
        expected.push((
            case,
            holds(git(&added.worktree.path, &["rev-parse", "HEAD"])),
        ));
    }
    git(&origin, &["checkout", "--quiet", "--detach"]);
    let detached = add(&root, &origin, Start::Head).await.unwrap();
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

// Covers: cleanup reads a branch whose changes the origin's HEAD has as
// merged however they got there: a merge commit, a squash merge, or a
// rebase merge (cherry-picks), each after HEAD changed a line next to the
// branch's, as a base moving on under a branch does, and a rebase merge
// whose lines HEAD edited again since; it reads one HEAD has only some of,
// none of, or conflicting changes for, as unmerged, and one whose merge
// commit adds changes of its own, though HEAD has each of its other
// commits' patches.
// Owner: session worktrees (git plumbing under the daemon's cleanup).
#[tokio::test]
async fn inspect_reads_squash_and_rebase_merges_as_merged() {
    // The branch edits lines 10 and 12. Line 8 is in the context of its
    // diff, so the origin editing it first gives a merge different patches;
    // line 3 is not.
    const NEXT_TO: usize = 8;
    // What a case does once the branch is ahead and the origin edited a
    // line; given the origin, the worktree, and the branch's two commits,
    // oldest first.
    type Merge = fn(&Path, &Path, &[String; 2]);
    let cases: [(&str, usize, Merge, bool); 8] = [
        (
            "merge commit",
            NEXT_TO,
            |origin, _, [_, last]| {
                git(
                    origin,
                    &["merge", "--quiet", "--no-ff", "--no-edit", last.as_str()],
                );
            },
            true,
        ),
        (
            "squash merge",
            NEXT_TO,
            |origin, _, [_, last]| {
                git(origin, &["merge", "--quiet", "--squash", last.as_str()]);
                git(origin, &["commit", "--quiet", "-m", "squash"]);
            },
            true,
        ),
        (
            "rebase merge",
            NEXT_TO,
            |origin, _, [first, last]| {
                git(origin, &["cherry-pick", first.as_str(), last.as_str()]);
            },
            true,
        ),
        (
            "rebase merge, its lines edited since",
            3,
            |origin, _, [first, last]| {
                git(origin, &["cherry-pick", first.as_str(), last.as_str()]);
                edit_line(origin, 10, "ten, edited again");
            },
            true,
        ),
        (
            "conflicts with HEAD",
            NEXT_TO,
            |origin, _, _| edit_line(origin, 10, "ten, another way"),
            false,
        ),
        (
            "half rebased",
            NEXT_TO,
            |origin, _, [first, _]| {
                git(origin, &["cherry-pick", first.as_str()]);
            },
            false,
        ),
        (
            "a merge commit adding changes, then rebased",
            3,
            |origin, worktree, [first, last]| {
                git(worktree, &["merge", "--quiet", "--no-commit", "main"]);
                std::fs::write(worktree.join("resolved.txt"), "new").unwrap();
                git(worktree, &["add", "resolved.txt"]);
                git(worktree, &["commit", "--quiet", "--no-edit"]);
                git(origin, &["cherry-pick", first.as_str(), last.as_str()]);
            },
            false,
        ),
        ("not merged", NEXT_TO, |_, _, _| {}, false),
    ];
    let mut found = Vec::new();
    let mut expected = Vec::new();
    for (case, origin_edits, merge, merged) in cases {
        let home = TempDir::new().unwrap();
        let origin = repository(home.path());
        edit_line(&origin, 1, "1");
        let added = add(&home.path().join("worktrees"), &origin, Start::Head)
            .await
            .unwrap();
        let path = &added.worktree.path;
        let commits = [(10, "ten"), (12, "twelve")].map(|(line, text)| {
            edit_line(path, line, text);
            git(path, &["rev-parse", "HEAD"])
        });
        edit_line(&origin, origin_edits, "moved on");
        merge(&origin, path, &commits);

        found.push((case, inspect(&added.worktree).await.unwrap()));
        let head = git(path, &["rev-parse", "HEAD"]);
        expected.push((
            case,
            match merged {
                true => Holds::Merged {
                    base: "main".to_owned(),
                    commit: head,
                },
                false => Holds::Unmerged {
                    base: "main".to_owned(),
                },
            },
        ));
    }

    assert_eq!(found, expected);
}

/// Commits, in the checkout at `dir`, `lines.txt` with line `number` of 20
/// set to `text`; the first commit writes the lines `1` to `20`.
fn edit_line(dir: &Path, number: usize, text: &str) {
    let file = dir.join("lines.txt");
    let mut lines: Vec<String> = match std::fs::read_to_string(&file) {
        Ok(content) => content.lines().map(str::to_owned).collect(),
        Err(_) => (1..=20).map(|line| line.to_string()).collect(),
    };
    lines[number - 1] = text.to_owned();
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();
    git(dir, &["add", "lines.txt"]);
    git(dir, &["commit", "--quiet", "-m", text]);
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
        let added = add(&root, &origin, Start::Head).await.unwrap();
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

// Covers: a worktree removed once squash-merged comes back at the origin's
// HEAD, on its branch made again or detached, when someone deleted the
// branch and git collected the commit it was removed at, which nothing
// else held.
// Owner: session worktrees (git plumbing under the daemon's cleanup).
#[tokio::test]
async fn a_removed_worktree_whose_commit_git_collected_comes_back_at_head() {
    let mut found = Vec::new();
    let mut expected = Vec::new();
    for checkout in [Checkout::Branch, Checkout::Detached] {
        let home = TempDir::new().unwrap();
        let origin = repository(home.path());
        let added = add(&home.path().join("worktrees"), &origin, Start::Head)
            .await
            .unwrap();
        let worktree = &added.worktree;
        edit_line(&worktree.path, 10, "ten");
        let removed_at = git(&worktree.path, &["rev-parse", "HEAD"]);
        git(
            &origin,
            &["merge", "--quiet", "--squash", worktree.branch.as_str()],
        );
        git(&origin, &["commit", "--quiet", "-m", "squash"]);
        remove(worktree).await.unwrap();
        git(
            &origin,
            &["branch", "--quiet", "-D", worktree.branch.as_str()],
        );
        git(&origin, &["reflog", "expire", "--expire=now", "--all"]);
        git(&origin, &["gc", "--quiet", "--prune=now"]);
        let objects = ["cat-file", "--batch-check", "--batch-all-objects"];
        let collected = !git(&origin, &objects).contains(removed_at.as_str());

        restore(worktree, &removed_at, &worktree.path, checkout)
            .await
            .unwrap();

        found.push((
            checkout,
            collected,
            git(&worktree.path, &["branch", "--show-current"]),
            git(&worktree.path, &["rev-parse", "HEAD"]),
        ));
        let branch = match checkout {
            Checkout::Branch => worktree.branch.clone(),
            Checkout::Detached => String::new(),
        };
        expected.push((checkout, true, branch, git(&origin, &["rev-parse", "HEAD"])));
    }

    assert_eq!(found, expected);
}

/// A clone `repo` in `home` of a bare `remote.git` whose `main` then gained
/// a commit the clone has not fetched, and that commit.
fn behind_its_remote(home: &Path) -> (std::path::PathBuf, String) {
    let seed = repository(home);
    let remote = home.join("remote.git");
    git(
        home,
        &["clone", "--quiet", "--bare", path(&seed), path(&remote)],
    );
    let repo = home.join("clone");
    git(home, &["clone", "--quiet", path(&remote), path(&repo)]);
    std::fs::write(seed.join("shared.txt"), "newer").unwrap();
    git(&seed, &["commit", "--quiet", "-am", "newer"]);
    git(&seed, &["push", "--quiet", path(&remote), "main"]);
    let newer = git(&seed, &["rev-parse", "HEAD"]);
    (crate::paths::canonicalize(&repo).unwrap(), newer)
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

// Covers: a worktree started from the remote starts at the branch the
// remote names its default now, fetched: whether the clone recorded it, a
// remote added by hand was never fetched, the recorded one is gone from the
// remote, or the remote's default moved while the old one stays; its branch
// tracks nothing, and the checkout's own branch does not move.
// Owner: session worktrees.
#[tokio::test]
async fn a_worktree_from_the_remote_starts_at_its_default_branch() {
    let mut found = Vec::new();
    let mut expected = Vec::new();
    for case in ["recorded", "unrecorded", "unfetched", "gone", "moved"] {
        let home = TempDir::new().unwrap();
        let (mut repo, newer) = behind_its_remote(home.path());
        let remote = home.path().join("remote.git");
        let (mut label, mut commit) = ("origin/main", newer);
        match case {
            "unrecorded" => {
                git(&repo, &["remote", "set-head", "origin", "--delete"]);
            }
            "unfetched" => {
                repo = home.path().join("by-hand");
                std::fs::create_dir_all(&repo).unwrap();
                git(&repo, &["init", "--quiet", "--initial-branch=main"]);
                git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "own"]);
                git(&repo, &["remote", "add", "origin", path(&remote)]);
                repo = crate::paths::canonicalize(&repo).unwrap();
            }
            "gone" => {
                let head = "refs/remotes/origin/HEAD";
                git(&repo, &["symbolic-ref", head, "refs/remotes/origin/master"]);
            }
            "moved" => {
                commit = git(&repo, &["rev-parse", "HEAD"]);
                git(&remote, &["branch", "trunk", commit.as_str()]);
                git(&remote, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
                label = "origin/trunk";
            }
            _ => {}
        }
        let before = git(&repo, &["rev-parse", "HEAD"]);

        let added = add(&home.path().join("worktrees"), &repo, Start::Remote)
            .await
            .unwrap();

        let tracking = format!("branch.{}.remote", added.worktree.branch);
        found.push((
            case,
            added.started,
            git(&added.worktree.path, &["rev-parse", "HEAD"]),
            git(
                &repo,
                &["config", "--default", "", "--get", tracking.as_str()],
            ),
            git(&repo, &["rev-parse", "HEAD"]),
        ));
        expected.push((
            case,
            Started::Remote(label.to_owned()),
            commit,
            String::new(),
            before,
        ));
    }

    assert_eq!(found, expected);
}

// Covers: a worktree asked to start from the remote starts from HEAD when
// the repository has no `origin`, and says it fell back when `origin`
// cannot be fetched.
// Owner: session worktrees.
#[tokio::test]
async fn a_worktree_from_the_remote_falls_back_to_head() {
    let mut found = Vec::new();
    let mut expected = Vec::new();
    for remote in [None, Some("/nonexistent/remote.git")] {
        let home = TempDir::new().unwrap();
        let repo = repository(home.path());
        if let Some(url) = remote {
            git(&repo, &["remote", "add", "origin", url]);
        }

        let added = add(&home.path().join("worktrees"), &repo, Start::Remote)
            .await
            .unwrap();

        let fell_back = matches!(added.started, Started::FellBack(_));
        found.push((
            remote,
            fell_back,
            git(&added.worktree.path, &["rev-parse", "HEAD"]),
        ));
        expected.push((remote, remote.is_some(), git(&repo, &["rev-parse", "HEAD"])));
    }

    assert_eq!(found, expected);
}

// Covers: cleanup reads a worktree started from the remote, with nothing
// new, as merged into the remote's default branch while the checkout's
// HEAD is still behind, so the daemon removes it rather than keeping it as
// unmerged forever; one with a commit neither has stays unmerged.
// Owner: session worktrees (git plumbing under the daemon's cleanup).
#[tokio::test]
async fn inspect_reads_a_worktree_the_remote_has_as_merged() {
    let home = TempDir::new().unwrap();
    let (repo, newer) = behind_its_remote(home.path());
    let root = home.path().join("worktrees");
    let clean = add(&root, &repo, Start::Remote).await.unwrap();
    let ahead = add(&root, &repo, Start::Remote).await.unwrap();
    edit_line(&ahead.worktree.path, 3, "three");

    assert_eq!(
        (
            inspect(&clean.worktree).await.unwrap(),
            inspect(&ahead.worktree).await.unwrap(),
        ),
        (
            Holds::Merged {
                base: "origin/main".to_owned(),
                commit: newer,
            },
            Holds::Unmerged {
                base: "main".to_owned(),
            },
        )
    );
}
