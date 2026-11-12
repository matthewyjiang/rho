//! `rho daemon` through the real binary. Here: its lifecycle (status,
//! single instance, and stop), and a dropped test sandbox stopping the
//! daemon a client in it started. The sessions it hosts are in
//! [`sessions`], the agent and setup a client opens one with in
//! [`setup`], changing their settings in [`configure`], and listing,
//! renaming, and deleting them in [`catalog`],
//! `rho attach` following them in [`attach`], and `rho run --detach`
//! starting them in [`detach`], and cleaning up the git worktrees they
//! work in in [`worktrees`]; the tool processes the daemon's stop ends
//! are in [`tools`], signing it in and out over its socket in [`auth`],
//! and the interactive logins it relays to clients, `rho login` among
//! them, in [`login`], signing in where no OS credential store answers in
//! [`headless`]; `rho mcp serve` driving its sessions is in [`mcp`],
//! and the sessions a session starts, as its children, in [`started`];
//! reading and changing settings over its socket is in [`config`];
//! re-executing the daemon in place, for an upgrade or `rho daemon
//! restart`, is in [`upgrade`]; what a client of another version hears at
//! the handshake is in [`skew`]; what a daemon takes from the client that
//! starts it, and each session's project trust, is in [`identity`]; what
//! `rho doctor` reports about it is in [`doctor`]; what `rho update` and
//! `rho uninstall` do about it is in [`install`]; a turn whose provider
//! cannot be reached is in [`unreachable`]; what a session records about
//! what follows a compaction is in [`compaction`].

use std::{
    fs::File,
    panic::AssertUnwindSafe,
    process::{Output, Stdio},
};

use pretty_assertions::assert_eq;
use serde_json::Value;

#[path = "support/daemon_sessions.rs"]
mod sessions;

#[path = "support/daemon_setup.rs"]
mod setup;

#[path = "support/daemon_configure.rs"]
mod configure;

#[path = "support/daemon_catalog.rs"]
mod catalog;

#[path = "support/daemon_attach.rs"]
mod attach;

#[path = "support/daemon_detach.rs"]
mod detach;

#[path = "support/git_repo.rs"]
mod git_repo;

#[path = "support/daemon_worktrees.rs"]
mod worktrees;

// Unix: the tool processes are shell commands, found by process group.
#[cfg(unix)]
#[path = "support/daemon_tools.rs"]
mod tools;

#[path = "support/daemon_auth.rs"]
mod auth;

#[path = "support/daemon_login.rs"]
mod login;

#[path = "support/daemon_headless.rs"]
mod headless;

// Unix: its sessions run shell commands, and its servers are shell scripts.
#[cfg(unix)]
#[path = "support/daemon_mcp.rs"]
mod mcp;

#[path = "support/daemon_config.rs"]
mod config;
// Unix: its sessions run shell commands.
#[cfg(unix)]
#[path = "support/daemon_started.rs"]
mod started;

// Unix: re-executing in place keeps the pid, lock and socket only on Unix.
#[cfg(unix)]
#[path = "support/daemon_upgrade.rs"]
mod upgrade;

// Unix: it holds turns open with the FIFOs `upgrade` makes.
#[cfg(unix)]
#[path = "support/daemon_skew.rs"]
mod skew;

// Unix: its hooks are shell scripts, and it reads `/proc`.
#[cfg(unix)]
#[path = "support/daemon_identity.rs"]
mod identity;

// Unix: its sessions run shell commands.
#[cfg(unix)]
#[path = "support/daemon_grants.rs"]
mod grants;

#[path = "support/daemon_doctor.rs"]
mod doctor;

#[path = "support/daemon_install.rs"]
mod install;

#[path = "support/daemon_unreachable.rs"]
mod unreachable;

#[path = "support/daemon_compaction.rs"]
mod compaction;

#[path = "support/key_checking_provider.rs"]
mod key_checking_provider;

#[path = "support/scripted_host.rs"]
mod scripted_host;

use sessions::Sandbox;

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// Covers: status, stop, and attach report a missing daemon without starting one; a
// running daemon reports its pid, refuses a second instance by name, and
// `rho daemon stop` makes it exit cleanly and remove its socket.
// Owner: CLI process (daemon lifecycle).
#[tokio::test]
async fn daemon_lifecycle_through_the_cli() {
    let sandbox = Sandbox::new();
    let output = |args: &[&str]| {
        sandbox
            .rho("fixed")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    };
    for args in [
        ["daemon", "status"],
        ["daemon", "stop"],
        ["attach", "session"],
    ] {
        let output = output(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert_eq!(stderr(&output), "Error: rho daemon is not running\n");
    }
    assert_eq!(
        sandbox.isolated.daemon_socket(),
        None,
        "status, stop, and attach never spawn"
    );

    let mut daemon = sandbox
        .rho("fixed")
        .args(["daemon", "run"])
        .spawn()
        .unwrap();
    let socket = sandbox
        .poll_until("the daemon socket", async || {
            sandbox.isolated.daemon_socket()
        })
        .await;

    let status = output(&["daemon", "status", "--json"]);
    assert!(status.status.success(), "{status:?}");
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["pid"], Value::from(daemon.id()));
    assert_eq!(status["socket"], Value::from(socket.to_str().unwrap()));

    let second = output(&["daemon", "run"]);
    assert_eq!(second.status.code(), Some(1), "{second:?}");
    assert_eq!(
        stderr(&second),
        format!(
            "Error: rho daemon is already running (pid {}, version {}, socket {})\n",
            daemon.id(),
            env!("CARGO_PKG_VERSION"),
            socket.display()
        )
    );

    let stop = output(&["daemon", "stop"]);
    assert!(stop.status.success(), "{stop:?}");
    let exit = sandbox
        .poll_until("the daemon to exit", async || daemon.try_wait().unwrap())
        .await;
    assert!(exit.success(), "daemon exit: {exit:?}");
    assert_eq!(
        sandbox.isolated.daemon_socket(),
        None,
        "socket removed on stop"
    );
}

// Covers: `rho daemon start` starts a daemon when none runs and exits once
// it answers; run again, it exits 0 and leaves that daemon the only one
// (#1730).
// Owner: CLI process (daemon lifecycle).
#[test]
fn daemon_start_is_idempotent() {
    let sandbox = Sandbox::new();
    let start = || {
        let output = sandbox
            .rho("fixed")
            .args(["daemon", "start"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        let pid = sandbox.isolated.daemon_pid();
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            pid,
        )
    };

    let first = start();
    let pid = first.2.expect("the started daemon's record");
    let second = start();

    let running = format!("rho daemon is running (pid {pid})\n");
    assert_eq!(
        (first, second),
        (
            (Some(0), running.clone(), Some(pid)),
            (Some(0), running, Some(pid)),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a frontend's `rho_protocol::LocalDaemon` starts the stopped daemon
// of an isolated rho home through `rho daemon start` and connects to it, and
// a later one finds that daemon running, starting nothing.
// Owner: rho-protocol LocalDaemon against the real CLI.
#[cfg(unix)]
#[tokio::test]
async fn a_frontend_starts_a_stopped_daemon_as_the_cli_does_then_finds_it() {
    use rho_protocol::{ClientHello, ClientInfo, LocalDaemon};

    let sandbox = Sandbox::new();
    let daemon = LocalDaemon::new(sandbox.isolated.rho_home())
        .with_xdg_runtime_dir(Some(sandbox.isolated.path().join("run")));
    let hello = || {
        ClientHello::frontend(ClientInfo {
            name: "local-daemon-test".into(),
            version: "0".into(),
        })
    };
    assert!(daemon.connect(hello()).await.unwrap().is_none(), "stopped");

    let (_client, _notifications, started) = daemon
        .connect_or_start(sandbox.rho("fixed"), hello())
        .await
        .unwrap();
    let pid = sandbox
        .isolated
        .daemon_pid()
        .expect("the started daemon's record");
    let no_rho = std::process::Command::new(sandbox.isolated.path().join("no-such-rho"));
    let (_client, _notifications, found) = daemon.connect_or_start(no_rho, hello()).await.unwrap();

    assert_eq!((started.daemon.pid, found.daemon.pid), (pid, pid));
}

// Covers: a client whose environment sets an unknown RHO_CREDENTIAL_STORE
// starts no daemon, which would refuse its sessions and sign-ins for it,
// and names the variable at once; with a daemon already running it
// connects, and that daemon refuses what needs the store.
// Owner: CLI process (daemon ensure-running path).
#[test]
fn an_unknown_credential_store_stops_only_a_daemon_start() {
    let sandbox = Sandbox::new();
    let start = |store: &str| {
        let output = sandbox
            .rho("fixed")
            .env("RHO_CREDENTIAL_STORE", store)
            .args(["daemon", "start"])
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        (output.status.code(), stderr(&output))
    };

    let (code, refused) = start("fiel");
    assert_eq!(
        (
            code,
            refused.contains("RHO_CREDENTIAL_STORE"),
            sandbox.isolated.daemon_pid()
        ),
        (Some(1), true, None),
        "{refused}"
    );
    assert_eq!(start("file").0, Some(0));
    assert_eq!(start("fiel"), (Some(0), String::new()));
}

// Covers: `rho daemon restart` returns once a daemon answers again, however
// the platform restarts one: in place, where a client waits in the socket's
// backlog, or as a new process, where the old one must leave nothing a
// client could connect to in between.
// Owner: CLI process (daemon restart).
#[test]
fn restart_returns_once_a_daemon_answers_again() {
    let sandbox = Sandbox::new();
    let run = |args: &[&str]| {
        sandbox
            .rho("fixed")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    };
    let started = run(&["daemon", "start"]);

    let restarted = run(&["daemon", "restart"]);
    let status = run(&["daemon", "status"]);

    assert_eq!(
        [started, restarted, status].map(|output| (output.status.code(), stderr(&output))),
        [
            (Some(0), String::new()),
            (Some(0), String::new()),
            (Some(0), String::new())
        ],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: dropping a sandbox stops the daemon a client in it started, so a
// test that fails, or forgets to stop it, leaves no daemon running.
// Owner: daemon test sandbox teardown (process).
#[test]
fn a_dropped_sandbox_stops_the_daemon_a_client_started() {
    let sandbox = Sandbox::new();
    // `rho run --detach` starts a daemon, which outlives it.
    let detached = sandbox
        .rho("fixed")
        .args(["run", "--detach", "hello"])
        .status()
        .unwrap();
    assert!(
        detached.success(),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
    // Kept open, it still probes the daemon's lock once the sandbox's
    // directory is deleted.
    let lock = File::open(sandbox.isolated.rho_home().join("daemon/daemon.lock")).unwrap();
    // Whether a shared lock is granted, which the daemon's exclusive one
    // refuses until it exits.
    let free = || fs2::FileExt::try_lock_shared(&lock).is_ok();
    assert!(!free(), "the daemon holds its lock");
    let pid = sandbox.isolated.daemon_pid().expect("the daemon's record");

    // Caught so the daemon is killed below if the teardown panics because it
    // failed to stop it.
    let teardown = std::panic::catch_unwind(AssertUnwindSafe(|| drop(sandbox)));
    let released = free();
    if !released {
        // The daemon must not outlive a failed test.
        kill(pid);
    }
    if let Err(panic) = teardown {
        std::panic::resume_unwind(panic);
    }
    assert!(released, "the daemon (pid {pid}) still holds its lock");
}

/// Ends process `pid` at once.
fn kill(pid: u32) {
    #[cfg(unix)]
    // SAFETY: `kill` takes no pointers.
    unsafe {
        libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL);
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
}
