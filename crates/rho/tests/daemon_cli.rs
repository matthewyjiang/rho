//! `rho daemon` through the real binary. Here: its lifecycle (status,
//! single instance, and stop), and a dropped test sandbox stopping the
//! daemon a client in it started. The sessions it hosts are in
//! [`sessions`], the agent and setup a client opens one with in
//! [`setup`], changing their settings in [`configure`], and listing,
//! renaming, and deleting them in [`catalog`],
//! `rho attach` following them in [`attach`], and `rho run --detach`
//! starting them in [`detach`]; the tool processes the daemon's stop ends
//! are in [`tools`], signing it in and out over its socket in [`auth`],
//! and the interactive logins it relays to clients, `rho login` among
//! them, in [`login`]; `rho mcp serve` driving its sessions is in [`mcp`];
//! reading and changing settings over its socket is in [`config`];
//! re-executing the daemon in place, for an upgrade or `rho daemon
//! restart`, is in [`upgrade`].
#![cfg(unix)]

use std::{
    fs::File,
    os::fd::AsRawFd,
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

#[path = "support/daemon_tools.rs"]
mod tools;

#[path = "support/daemon_auth.rs"]
mod auth;

#[path = "support/daemon_login.rs"]
mod login;

#[path = "support/daemon_mcp.rs"]
mod mcp;

#[path = "support/daemon_config.rs"]
mod config;
#[path = "support/daemon_upgrade.rs"]
mod upgrade;

#[path = "support/key_checking_provider.rs"]
mod key_checking_provider;

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

// Covers: a client whose environment sets an unknown RHO_CREDENTIAL_STORE
// refuses to start a daemon, which would inherit it and stop, naming the
// variable at once instead of waiting out the start timeout; with a daemon
// already running, which never sees the client's environment, it connects.
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
    // SAFETY: `flock` takes only a descriptor, which `lock` keeps open.
    let free = || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) == 0 };
    assert!(!free(), "the daemon holds its lock");
    let pid = sandbox.isolated.daemon_pid().expect("the daemon's record");

    // Caught so the daemon is killed below if the teardown panics because it
    // failed to stop it.
    let teardown = std::panic::catch_unwind(AssertUnwindSafe(|| drop(sandbox)));
    let released = free();
    if !released {
        // SAFETY: `kill` takes no pointers. The daemon must not outlive a
        // failed test.
        unsafe { libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
    }
    if let Err(panic) = teardown {
        std::panic::resume_unwind(panic);
    }
    assert!(released, "the daemon (pid {pid}) still holds its lock");
}
