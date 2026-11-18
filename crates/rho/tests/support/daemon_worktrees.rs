//! Cleaning up session worktrees through the real binary (#1673):
//! `rho worktrees prune` removes a worktree that is clean and merged,
//! keeping the branch, keeps every other one, and never touches one a
//! session uses. A session reopened or read after gets its worktree back
//! at the commit it was removed at, on the kept branch or the branch made
//! again; one a person removed stays removed. When the daemon's own pass
//! runs is unit-tested beside it.

use std::{
    path::Path,
    process::{Output, Stdio},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionObserve},
    Client, JobListParams, SessionId, SessionObserveParams, SessionWorktree,
};

use super::{
    detach::{attach, detach, session_of},
    git_repo::{git, init_repo},
    sessions::{settled_job, Sandbox},
};

/// What a daemon these tests start unloads its idle sessions after, so a
/// finished session's worktree is not in use for long.
const UNLOAD_SOON: [&str; 2] = ["--unload-idle-after", "100ms"];

/// `rho worktrees prune` with `args` against the fixture in `mode`.
fn prune(sandbox: &Sandbox, mode: &str, args: &[&str]) -> Output {
    sandbox
        .rho(mode)
        .args(["worktrees", "prune"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

/// A finished `rho run --detach --worktree` session, once the daemon
/// unloaded it, and its worktree.
async fn finished_worktree_session(
    sandbox: &Sandbox,
    client: &Client,
) -> (SessionId, SessionWorktree) {
    let session = session_of(&detach(sandbox, "fixed", &["--worktree", "hello"]));
    let job = settled_job(sandbox, client, &session).await;
    sandbox.wait_unloaded(&session, 1).await;
    (session, job.worktree.expect("the job names its worktree"))
}

/// Commits a file `name` in `worktree`. Two worktrees committing the same
/// file within a second would make the same commit.
fn commit(worktree: &Path, name: &str) {
    std::fs::write(worktree.join(name), "work").unwrap();
    git(worktree, &["add", name]);
    git(worktree, &["commit", "--quiet", "-m", name]);
}

/// The line `rho worktrees prune` prints for `worktree`.
fn line(action: &str, worktree: &SessionWorktree, reason: &str) -> String {
    format!(
        "{action:<12} {} {}: {reason}",
        worktree.branch,
        worktree.path.display()
    )
}

/// `output`'s stdout lines, sorted: the daemon reports worktrees in its
/// job order, which jobs created in the same second break by id.
fn sorted_lines(output: &Output) -> Vec<String> {
    let mut lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

// Covers: `rho worktrees prune --dry-run` lists which worktrees would go
// and why, changing nothing; `rho worktrees prune` then removes the
// worktree that is clean and whose commit is squash-merged into the
// repository's HEAD, its branch kept, and keeps the one with an untracked
// file and the one with an unmerged commit.
// Owner: daemon worktree cleanup through `rho worktrees prune`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_removes_only_clean_merged_worktrees_and_keeps_their_branches() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    let client = sandbox.start_daemon_with("fixed", &UNLOAD_SOON).await;
    let (_, merged) = finished_worktree_session(&sandbox, &client).await;
    let (_, dirty) = finished_worktree_session(&sandbox, &client).await;
    let (_, unmerged) = finished_worktree_session(&sandbox, &client).await;
    commit(&merged.path, "merged.txt");
    // Squashed, as a forge merges a pull request: the branch is no
    // ancestor of HEAD.
    git(
        &sandbox.workspace,
        &["merge", "--quiet", "--squash", merged.branch.as_str()],
    );
    git(&sandbox.workspace, &["commit", "--quiet", "-m", "squash"]);
    std::fs::write(dirty.path.join("notes.txt"), "draft").unwrap();
    commit(&unmerged.path, "unmerged.txt");
    let on_disk = || [&merged, &dirty, &unmerged].map(|worktree| worktree.path.exists());

    let dry_run = prune(&sandbox, "fixed", &["--dry-run"]);
    let after_dry_run = on_disk();
    let pruned = prune(&sandbox, "fixed", &[]);

    let report = |merged_action: &str| {
        let mut lines = vec![
            line(
                merged_action,
                &merged,
                "clean and merged into main; the branch stays",
            ),
            line("kept", &dirty, "uncommitted or untracked changes"),
            line("kept", &unmerged, "not merged into main"),
        ];
        lines.sort();
        lines
    };
    assert_eq!(
        (
            (dry_run.status.code(), sorted_lines(&dry_run), after_dry_run),
            (pruned.status.code(), sorted_lines(&pruned), on_disk()),
            git(
                &sandbox.workspace,
                &["branch", "--list", merged.branch.as_str()]
            ),
        ),
        (
            (Some(0), report("would remove"), [true, true, true]),
            (Some(0), report("removed"), [false, true, true]),
            merged.branch.clone(),
        ),
        "stderr: {}{}\nlog:\n{}",
        String::from_utf8_lossy(&dry_run.stderr),
        String::from_utf8_lossy(&pruned.stderr),
        sandbox.isolated.daemon_log()
    );
}

/// `rho worktrees status` against the fixture in `mode`.
fn status(sandbox: &Sandbox, mode: &str) -> Output {
    sandbox
        .rho(mode)
        .args(["worktrees", "status"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

/// Sets when `worktree`'s checkout last moved, as cleanup reads it, to two
/// days ago.
fn unused_for_two_days(worktree: &Path) {
    let admin = git(worktree, &["rev-parse", "--absolute-git-dir"]);
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 60 * 60);
    for file in ["HEAD", "logs/HEAD"] {
        std::fs::File::options()
            .write(true)
            .open(Path::new(&admin).join(file))
            .and_then(|file| file.set_modified(then))
            .unwrap();
    }
}

// Covers: worktrees under the rho home that no session owns, as `git
// worktree add` there and a foreground `rho run --worktree` make, are
// cleaned up as a session's are: `rho worktrees status` lists each with
// its size and whether a prune removes it, then the count, and `rho
// worktrees prune` removes the clean one whose branch was squash-merged
// and then edited again, so it no longer merges cleanly, keeping its
// branch; it keeps the one a process works in, the one `git worktree lock`
// locked, which the dry run already says, and the one added just now, as a
// session's still opening is before its job names it.
// Owner: daemon worktree cleanup through `rho worktrees status` and
// `rho worktrees prune`.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktrees_no_session_owns_are_listed_and_pruned() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    let _client = sandbox.start_daemon("fixed").await;
    let root = sandbox.isolated.rho_home().join("worktrees");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let add = |name: &str| {
        let path = root.join(name);
        let at = path.to_string_lossy().into_owned();
        git(
            &sandbox.workspace,
            &["worktree", "add", "--quiet", "-b", name, at.as_str()],
        );
        SessionWorktree {
            branch: name.to_owned(),
            path,
            origin: sandbox.workspace.clone(),
        }
    };
    let merged = add("merged");
    let occupied = add("occupied");
    let fresh = add("fresh");
    let locked = add("locked");
    git(
        &sandbox.workspace,
        &["worktree", "lock", locked.path.to_str().unwrap()],
    );
    // Two commits, squashed into one, then edited again: the branch is no
    // ancestor, matches no single commit's patch, and conflicts.
    commit(&merged.path, "merged.txt");
    commit(&merged.path, "more.txt");
    git(
        &sandbox.workspace,
        &["merge", "--quiet", "--squash", merged.branch.as_str()],
    );
    git(&sandbox.workspace, &["commit", "--quiet", "-m", "squash"]);
    std::fs::write(sandbox.workspace.join("merged.txt"), "edited since").unwrap();
    git(&sandbox.workspace, &["commit", "--quiet", "-am", "edit"]);
    unused_for_two_days(&merged.path);
    unused_for_two_days(&occupied.path);
    unused_for_two_days(&locked.path);
    // Waits on its stdin, so it works there until the checks are done.
    let mut occupant = std::process::Command::new("cat")
        .current_dir(&occupied.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();

    let listed = status(&sandbox, "fixed");
    let pruned = prune(&sandbox, "fixed", &[]);
    drop(occupant.stdin.take());
    occupant.wait().unwrap();

    // Each row without its size, which the paths in `.git` files vary,
    // and whether it had one; then the count line, up to the sizes.
    let stdout = String::from_utf8_lossy(&listed.stdout);
    let (rows, summary) = stdout.split_once("\n\n").unwrap_or((&stdout, ""));
    let mut rows: Vec<(bool, String)> = rows
        .lines()
        .map(|row| {
            let (size, rest) = row.split_at(10);
            (size.trim() != "?", rest.trim_start().to_owned())
        })
        .collect();
    rows.sort();
    let unowned = "; no session owns it";
    let merged_reason = format!("clean and merged into main; the branch stays{unowned}");
    let occupied_reason = format!("a process works in it{unowned}");
    let fresh_reason =
        format!("its checkout moved or a job in it changed within the last day{unowned}");
    let locked_reason = format!("locked with `git worktree lock`{unowned}");
    let row = |action: &str, worktree: &SessionWorktree, reason: &str| {
        let (branch, path) = (&worktree.branch, worktree.path.display());
        (true, format!("{action:<9} {branch} {path}: {reason}"))
    };
    let mut expected_rows = vec![
        row("kept", &fresh, &fresh_reason),
        row("kept", &locked, &locked_reason),
        row("kept", &occupied, &occupied_reason),
        row("removable", &merged, &merged_reason),
    ];
    expected_rows.sort();
    let mut expected_pruned = vec![
        line("kept", &fresh, &fresh_reason),
        line("kept", &locked, &locked_reason),
        line("kept", &occupied, &occupied_reason),
        line("removed", &merged, &merged_reason),
    ];
    expected_pruned.sort();
    assert_eq!(
        (
            listed.status.code(),
            rows,
            summary.split(", ").next(),
            summary
                .split("; ")
                .nth(1)
                .and_then(|part| part.split(", ").next()),
            pruned.status.code(),
            sorted_lines(&pruned),
            [&merged.path, &occupied.path, &fresh.path, &locked.path].map(|path| path.exists()),
            git(&sandbox.workspace, &["branch", "--list", "merged"]),
        ),
        (
            Some(0),
            expected_rows,
            Some("4 worktrees"),
            Some("1 removable"),
            Some(0),
            expected_pruned,
            [false, true, true, true],
            "merged".to_owned(),
        ),
        "stderr: {}{}\nlog:\n{}",
        String::from_utf8_lossy(&listed.stderr),
        String::from_utf8_lossy(&pruned.stderr),
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho worktrees prune` keeps the worktree of a session whose turn
// is running, though it is clean and merged, and says why.
// Owner: daemon worktree cleanup through `rho worktrees prune`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_leaves_a_running_sessions_worktree_alone() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    // The fixture's turns run 30 s, past this test.
    let client = sandbox.start_daemon("delay").await;
    let session = session_of(&detach(&sandbox, "delay", &["--worktree", "hello"]));
    let worktree = client
        .request::<JobList>(&JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .find(|job| job.session.as_ref() == Some(&session))
        .and_then(|job| job.worktree)
        .expect("the daemon lists the session's worktree");

    let pruned = prune(&sandbox, "delay", &[]);

    assert_eq!(
        (
            pruned.status.code(),
            sorted_lines(&pruned),
            worktree.path.exists(),
        ),
        (
            Some(0),
            vec![line("kept", &worktree, "its session is loaded or running")],
            true,
        ),
        "stderr: {}\nlog:\n{}",
        String::from_utf8_lossy(&pruned.stderr),
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho worktrees prune` removes a finished session's clean, merged
// worktree, and attaching to the session later adds it back on its kept
// branch, so the session works where it did.
// Owner: daemon worktree cleanup through `rho worktrees prune` and the
// daemon's reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reopened_session_gets_its_removed_worktree_back() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    let client = sandbox.start_daemon_with("fixed", &UNLOAD_SOON).await;
    let (session, worktree) = finished_worktree_session(&sandbox, &client).await;

    let pruned = prune(&sandbox, "fixed", &[]);
    let removed = !worktree.path.exists();
    let attached = attach(&sandbox, "fixed", &session);

    assert_eq!(
        (
            pruned.status.code(),
            sorted_lines(&pruned),
            removed,
            attached.status.code(),
            git(&worktree.path, &["branch", "--show-current"]),
        ),
        (
            Some(0),
            vec![line(
                "removed",
                &worktree,
                "clean and merged into main; the branch stays"
            )],
            true,
            Some(0),
            worktree.branch.clone(),
        ),
        "stderr: {}\n{}\nlog:\n{}",
        String::from_utf8_lossy(&pruned.stderr),
        String::from_utf8_lossy(&attached.stderr),
        sandbox.isolated.daemon_log()
    );
}

// Covers: reading a session whose worktree was pruned and whose merged
// branch someone then deleted, as `rho mcp serve`'s `read` does, adds the
// worktree back on that branch, made again at the commit it was removed at.
// Owner: daemon worktree cleanup through `rho/session/observe`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reading_a_session_brings_back_its_worktree_after_its_branch_was_deleted() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    let client = sandbox.start_daemon_with("fixed", &UNLOAD_SOON).await;
    let (session, worktree) = finished_worktree_session(&sandbox, &client).await;
    let pruned = prune(&sandbox, "fixed", &[]);
    git(
        &sandbox.workspace,
        &["branch", "--quiet", "-d", worktree.branch.as_str()],
    );

    let observed = client
        .request::<SessionObserve>(&SessionObserveParams {
            session,
            after: None,
        })
        .await;

    assert_eq!(
        (
            pruned.status.code(),
            observed.map(drop).map_err(|error| error.to_string()),
            git(&worktree.path, &["branch", "--show-current"]),
            git(&worktree.path, &["rev-parse", "HEAD"]),
        ),
        (
            Some(0),
            Ok(()),
            worktree.branch.clone(),
            git(&sandbox.workspace, &["rev-parse", "HEAD"]),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: the daemon never adds back a worktree a person removed, nor makes
// again the branch they deleted: reading the session then fails, as it did
// before cleanup existed, instead of resuming it on unrelated code.
// Owner: daemon worktree cleanup through `rho/session/observe`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worktree_a_person_removed_stays_removed() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    let client = sandbox.start_daemon_with("fixed", &UNLOAD_SOON).await;
    let (session, worktree) = finished_worktree_session(&sandbox, &client).await;
    let path = worktree.path.to_string_lossy().into_owned();
    git(&sandbox.workspace, &["worktree", "remove", path.as_str()]);
    git(
        &sandbox.workspace,
        &["branch", "--quiet", "-D", worktree.branch.as_str()],
    );

    let observed = client
        .request::<SessionObserve>(&SessionObserveParams {
            session,
            after: None,
        })
        .await;

    assert_eq!(
        (
            observed.is_err(),
            worktree.path.exists(),
            git(
                &sandbox.workspace,
                &["branch", "--list", worktree.branch.as_str()]
            ),
        ),
        (true, false, String::new()),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: once cleanup removed a worktree and its branch is checked out in
// the repository's own checkout, reading the session still works, on a
// checkout of its commit removed again after the read, while reopening it
// for turns fails instead of running them off the branch.
// Owner: daemon worktree cleanup through `rho/session/observe` and
// `rho attach`'s reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_branch_checked_out_elsewhere_lets_a_read_but_not_turns_restore() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    let client = sandbox.start_daemon_with("fixed", &UNLOAD_SOON).await;
    let (session, worktree) = finished_worktree_session(&sandbox, &client).await;
    let pruned = prune(&sandbox, "fixed", &[]);
    git(
        &sandbox.workspace,
        &["checkout", "--quiet", worktree.branch.as_str()],
    );

    let observed = client
        .request::<SessionObserve>(&SessionObserveParams {
            session: session.clone(),
            after: None,
        })
        .await;
    let after_read = worktree.path.exists();
    let attached = attach(&sandbox, "fixed", &session);

    assert_eq!(
        (
            pruned.status.code(),
            observed.map(drop).map_err(|error| error.to_string()),
            after_read,
            attached.status.success(),
            worktree.path.exists(),
        ),
        (Some(0), Ok(()), false, false, false),
        "{}\nlog:\n{}",
        String::from_utf8_lossy(&attached.stderr),
        sandbox.isolated.daemon_log()
    );
}

// Covers: a checkout the daemon added back and never recorded, as one that
// died mid-read leaves, does not block the session: a read removes that
// clean leftover and adds the worktree back as usual.
// Owner: daemon worktree cleanup through `rho/session/observe`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkout_left_by_a_crash_does_not_block_reading() {
    let sandbox = Sandbox::new();
    init_repo(&sandbox.workspace);
    let client = sandbox.start_daemon_with("fixed", &UNLOAD_SOON).await;
    let (session, worktree) = finished_worktree_session(&sandbox, &client).await;
    let pruned = prune(&sandbox, "fixed", &[]);
    let path = worktree.path.to_string_lossy().into_owned();
    git(
        &sandbox.workspace,
        &["worktree", "add", "--quiet", "--detach", path.as_str()],
    );

    let observed = client
        .request::<SessionObserve>(&SessionObserveParams {
            session,
            after: None,
        })
        .await;

    assert_eq!(
        (
            pruned.status.code(),
            observed.map(drop).map_err(|error| error.to_string()),
            git(&worktree.path, &["branch", "--show-current"]),
        ),
        (Some(0), Ok(()), worktree.branch.clone()),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
