//! `rho run --detach` through the real binary: it starts a daemon when none
//! runs, leaves the turn running there for `rho attach`, has the daemon
//! enforce `--timeout`, passes `--add-dir` on as the session's setup, and
//! with `--worktree` runs in a git worktree the daemon adds for it. Which
//! flags it refuses is `cli_config`'s, tested there.

use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

use pretty_assertions::assert_eq;
use rho_protocol::{JobState, Operation, PathScope, SessionId};

use super::{
    sessions::{settled_job, Sandbox, REPLY},
    setup::{asked_operation, extra_dir},
};

/// `rho run --detach` with `args` against the fixture in `mode`.
fn detach(sandbox: &Sandbox, mode: &str, args: &[&str]) -> Output {
    sandbox
        .rho(mode)
        .args(["run", "--detach"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

/// The session id a successful detach printed.
fn session_of(detached: &Output) -> SessionId {
    let stdout = String::from_utf8(detached.stdout.clone()).unwrap();
    assert_eq!(
        detached.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&detached.stderr)
    );
    SessionId::new(stdout.trim_end().to_owned())
}

fn attach(sandbox: &Sandbox, mode: &str, session: &SessionId) -> Output {
    sandbox
        .rho(mode)
        .args(["attach", session.as_str()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

// Covers: with no daemon running, `rho run --detach` starts one, prints the
// session's id once the prompt is saved, and exits 0; the turn runs on
// there, and `rho attach` shows the conversation and its answer.
// Owner: rho run --detach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_run_starts_the_daemon_and_runs_on_there() {
    // Dropping the sandbox stops the daemon the detach starts.
    let sandbox = Sandbox::new();
    let session = session_of(&detach(&sandbox, "fixed", &["hello"]));
    let attached = attach(&sandbox, "fixed", &session);

    assert_eq!(
        (
            attached.status.code(),
            String::from_utf8(attached.stdout).unwrap()
        ),
        (Some(0), format!("> hello\n{REPLY}\n")),
        "{}\nlog:\n{}",
        String::from_utf8_lossy(&attached.stderr),
        sandbox.isolated.daemon_log()
    );
}

// Covers: the daemon cancels a detached turn still running at its
// `--timeout`; the job ends timed out, and `rho attach` exits as `rho run
// --timeout` does, 124, naming the limit.
// Owner: rho run --detach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_times_a_detached_turn_out() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("delay").await;
    let session = session_of(&detach(&sandbox, "delay", &["--timeout", "2s", "hello"]));
    let job = settled_job(&sandbox, &client, &session).await;
    let attached = attach(&sandbox, "delay", &session);

    assert_eq!(
        (
            job.state,
            attached.status.code(),
            String::from_utf8(attached.stderr)
                .unwrap()
                .lines()
                .last()
                .map(str::to_owned),
        ),
        (
            JobState::TimedOut,
            Some(124),
            Some("Error: the turn timed out at its limit of 2s".to_owned()),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: the daemon resolves a detached prompt's slash command as `rho
// run` does, and one it does not run, such as the TUI's `/model`, fails the
// turn before its prompt is saved, which the detach reports as a
// configuration error, exit 2.
// Owner: rho run --detach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_command_the_daemon_cannot_run_is_refused() {
    let sandbox = Sandbox::new();
    let _client = sandbox.start_daemon("fixed").await;
    let refused = detach(&sandbox, "fixed", &["/model"]);

    assert_eq!(
        (
            refused.status.code(),
            String::from_utf8(refused.stderr).unwrap()
        ),
        (
            Some(2),
            "Error: could not run /model: not available in daemon sessions\n".to_owned()
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho run --detach --add-dir` resolves a relative directory against
// the current directory, as a foreground run does, and the daemon session
// it opens takes it: a write there is to a root the user granted.
// Owner: rho run --detach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_run_passes_its_added_directories_to_the_daemon() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("supervised");
    let extra = extra_dir(&sandbox);
    let note = extra.join("note.txt");
    // The fixture makes the call the prompt spells out as JSON.
    let client = sandbox.start_daemon("tool-call").await;
    let call = serde_json::json!({
        "id": "edit",
        "name": "write",
        "arguments": {"path": note, "content": "granted"},
    });
    let prompt = call.to_string();
    let session = session_of(&detach(
        &sandbox,
        "tool-call",
        &["--add-dir", "../extra", prompt.as_str()],
    ));

    assert_eq!(
        asked_operation(&sandbox, &client, &session).await,
        Operation::WritePath {
            path: note,
            scope: PathScope::GrantedRoot { root: extra },
        },
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// Runs `git args` in `dir` with an identity and no signing, whatever the
/// machine's config, and returns its trimmed stdout.
pub(super) fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Makes `dir` a git repository whose one commit has `shared.txt` say
/// "base".
pub(super) fn init_repo(dir: &Path) {
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(dir.join("shared.txt"), "base").unwrap();
    git(dir, &["add", "shared.txt"]);
    git(dir, &["commit", "--quiet", "-m", "base"]);
}

// Covers: two `rho run --detach --worktree` runs in one repository write
// different text to the same tracked file, each in a worktree the daemon
// added on a branch of its own: each edit stays in its own worktree on its
// own branch, where in one checkout the second would overwrite the first,
// and the repository's own checkout keeps its file and stays clean
// (#1673).
// Owner: rho run --detach CLI (process contract) over daemon worktrees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_runs_in_worktrees_keep_their_conflicting_edits_apart() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    init_repo(&sandbox.workspace);
    let client = sandbox.start_daemon("tool-call").await;
    let write = |content: &str| {
        serde_json::json!({
            "id": "edit",
            "name": "write",
            "arguments": {"path": "shared.txt", "content": content},
        })
        .to_string()
    };
    let first = session_of(&detach(
        &sandbox,
        "tool-call",
        &["--worktree", write("first").as_str()],
    ));
    let second = session_of(&detach(
        &sandbox,
        "tool-call",
        &["--worktree", write("second").as_str()],
    ));

    let mut runs = Vec::new();
    for session in [&first, &second] {
        let job = settled_job(&sandbox, &client, session).await;
        let worktree = job.worktree.expect("the job names its worktree");
        runs.push((
            job.state,
            job.workspace == worktree.path,
            worktree.origin == sandbox.workspace,
            git(&worktree.path, &["branch", "--show-current"]) == worktree.branch,
            std::fs::read_to_string(worktree.path.join("shared.txt")).unwrap(),
        ));
    }
    let checkout = (
        std::fs::read_to_string(sandbox.workspace.join("shared.txt")).unwrap(),
        git(&sandbox.workspace, &["branch", "--show-current"]),
        git(&sandbox.workspace, &["status", "--porcelain"]),
    );

    assert_eq!(
        (runs, checkout),
        (
            vec![
                (JobState::Completed, true, true, true, "first".to_owned()),
                (JobState::Completed, true, true, true, "second".to_owned()),
            ],
            ("base".to_owned(), "main".to_owned(), String::new()),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho run --detach --worktree` outside every git repository opens
// no session and exits as a configuration error, exit 2, saying why.
// Owner: rho run --detach CLI (process contract) over daemon worktrees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_worktree_run_outside_git_is_refused() {
    let sandbox = Sandbox::new();
    let refused = detach(&sandbox, "fixed", &["--worktree", "hello"]);

    assert_eq!(
        (
            refused.status.code(),
            String::from_utf8(refused.stderr).unwrap()
        ),
        (
            Some(2),
            format!(
                "Error: could not open a daemon session: rho daemon rejected rho/session/new: \
                 could not open session in a worktree: {} is not in a git repository, so no \
                 worktree can be added for it (code -32008)\n",
                sandbox.workspace.display()
            )
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
