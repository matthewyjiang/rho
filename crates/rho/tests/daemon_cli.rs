//! `rho daemon` through the real binary: its lifecycle (status, single
//! instance, and stop) and the test sandbox stopping it here, the sessions
//! it hosts in [`sessions`],
//! changing their settings in [`configure`], and listing, renaming, and
//! deleting them in [`catalog`],
//! `rho attach` following them in [`attach`], and `rho run --detach`
//! starting them in [`detach`]; the tool processes the daemon's stop ends
//! are in [`tools`], signing it in and out over its socket in [`auth`],
//! and the interactive logins it relays to clients, `rho login` among
//! them, in [`login`].
#![cfg(unix)]

use std::process::{Output, Stdio};

use pretty_assertions::assert_eq;
use serde_json::Value;

#[path = "support/daemon_sessions.rs"]
mod sessions;

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

// Covers: a sandbox that drops while its test panics stops the daemon a
// client in it started, so a failing test leaves no daemon running.
// Owner: daemon test sandbox teardown (process).
#[test]
fn a_sandbox_dropped_in_a_panic_stops_the_daemon_a_client_started() {
    let mut pid = None;
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sandbox = Sandbox::new();
        // `rho run --detach` starts a daemon, which outlives it.
        let detached = sandbox
            .rho("fixed")
            .args(["run", "--detach", "hello"])
            .status()
            .unwrap();
        assert!(detached.success(), "log:\n{}", sandbox.daemon_log());
        pid = sandbox.isolated.daemon_pid();
        panic!("the test fails while the daemon runs");
    }));
    let Some(pid) = pid else {
        std::panic::resume_unwind(unwound.expect_err("the test panics"));
    };
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: signal 0 only checks that `pid` exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    if alive {
        // SAFETY: as above; the daemon must not outlive a failed check.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    assert_eq!((unwound.is_err(), alive), (true, false));
}
