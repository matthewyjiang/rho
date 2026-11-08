//! `rho run --detach` through the real binary: it starts a daemon when none
//! runs, leaves the turn running there for `rho attach`, and has the daemon
//! enforce `--timeout`. Which flags it refuses is `cli_config`'s, tested
//! there.

use std::process::{Output, Stdio};

use pretty_assertions::assert_eq;
use rho_protocol::{JobState, SessionId};

use super::sessions::{settled_job, Sandbox, REPLY};

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
// run` does, and one it does not run yet fails the turn before its prompt
// is saved, which the detach reports as a configuration error, exit 2.
// Owner: rho run --detach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_command_the_daemon_cannot_run_is_refused() {
    let sandbox = Sandbox::new();
    let _client = sandbox.start_daemon("fixed").await;
    let refused = detach(&sandbox, "fixed", &["/compact"]);

    assert_eq!(
        (
            refused.status.code(),
            String::from_utf8(refused.stderr).unwrap()
        ),
        (
            Some(2),
            "Error: could not run /compact: not available in daemon sessions\n".to_owned()
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
