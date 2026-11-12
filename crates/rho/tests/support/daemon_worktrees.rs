//! Cleaning up session worktrees through the real binary (#1673):
//! `rho worktrees prune` removes a worktree that is clean and merged,
//! keeping the branch, keeps every other one, and never touches one a
//! session uses. A session reopened or read after gets its worktree back,
//! from the kept branch or, once someone deleted it, from the repository's
//! HEAD. When the daemon's own pass runs is unit-tested beside it.

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
// worktree that is clean and whose commit is merged into the repository's
// HEAD, its branch kept, and keeps the one with an untracked file and the
// one with an unmerged commit.
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
    git(
        &sandbox.workspace,
        &["merge", "--quiet", "--ff-only", merged.branch.as_str()],
    );
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
// worktree back on that branch, made again from the repository's HEAD.
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
