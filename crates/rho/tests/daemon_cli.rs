//! `rho daemon` through the real binary: its lifecycle (status, single
//! instance, and stop) here, and the threads it hosts in [`threads`].
#![cfg(unix)]

use std::{
    process::{Output, Stdio},
    time::Duration,
};

use pretty_assertions::assert_eq;
use serde_json::Value;

#[path = "support/daemon_harness.rs"]
mod harness;

#[path = "support/daemon_threads.rs"]
mod threads;

use harness::{DaemonProcess, DaemonSandbox};

/// Failure bound for each wait on the daemon process. Startup and shutdown
/// are local file and socket operations; this only stops a hung test.
const DEADLINE: Duration = Duration::from_secs(30);

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// Covers: status and stop report a missing daemon without starting one; a
// running daemon reports its pid, refuses a second instance by name, and
// `rho daemon stop` makes it exit cleanly and remove its socket.
// Owner: CLI process (daemon lifecycle).
#[tokio::test]
async fn daemon_lifecycle_through_the_cli() {
    let sandbox = DaemonSandbox::new();
    let output = |args: &[&str]| sandbox.rho().args(args).output().unwrap();
    for args in [["daemon", "status"], ["daemon", "stop"]] {
        let output = output(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert_eq!(stderr(&output), "Error: rho daemon is not running\n");
    }
    assert!(!sandbox.socket().exists(), "status and stop never spawn");

    let mut daemon = DaemonProcess(
        sandbox
            .rho()
            .args(["daemon", "run"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    sandbox
        .poll_until("the daemon socket", DEADLINE, async || {
            sandbox.socket().exists().then_some(())
        })
        .await;

    let status = output(&["daemon", "status", "--json"]);
    assert!(status.status.success(), "{status:?}");
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["pid"], Value::from(daemon.0.id()));
    assert_eq!(
        status["socket"],
        Value::from(sandbox.socket().to_str().unwrap())
    );

    let second = output(&["daemon", "run"]);
    assert_eq!(second.status.code(), Some(1), "{second:?}");
    assert_eq!(
        stderr(&second),
        format!(
            "Error: rho daemon is already running (pid {}, version {})\n",
            daemon.0.id(),
            env!("CARGO_PKG_VERSION")
        )
    );

    let stop = output(&["daemon", "stop"]);
    assert!(stop.status.success(), "{stop:?}");
    let exit = sandbox
        .poll_until("the daemon to exit", DEADLINE, async || {
            daemon.0.try_wait().unwrap()
        })
        .await;
    assert!(exit.success(), "daemon exit: {exit:?}");
    assert!(!sandbox.socket().exists(), "socket removed on stop");
}
