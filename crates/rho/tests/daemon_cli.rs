//! `rho daemon` lifecycle through the real binary: status, single instance,
//! and stop.
#![cfg(unix)]

use std::{
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;

/// Failure bound for each wait on the daemon process. Startup and shutdown
/// are local file and socket operations; this only stops a hung test.
const DEADLINE: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            root: TempDir::new().unwrap(),
        }
    }

    fn socket(&self) -> PathBuf {
        self.root.path().join("home/run/daemon.sock")
    }

    /// `rho` with its data root and runtime directory inside the sandbox.
    /// Only the child's environment changes.
    fn rho(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
        command
            .args(args)
            .env("HOME", self.root.path())
            .env("RHO_HOME", self.root.path().join("home"))
            .env("XDG_RUNTIME_DIR", self.root.path().join("run"))
            .env_remove("JOURNAL_STREAM")
            .env_remove("RHO_LOG")
            .stdin(Stdio::null());
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        self.rho(args).output().unwrap()
    }
}

/// Kills the daemon if the test fails before stopping it.
struct DaemonProcess(Child);

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < DEADLINE, "timed out waiting for {what}");
        thread::sleep(POLL_INTERVAL);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// Covers: status and stop report a missing daemon without starting one; a
// running daemon reports its pid, refuses a second instance by name, and
// `rho daemon stop` makes it exit cleanly and remove its socket.
// Owner: CLI process (daemon lifecycle).
#[test]
fn daemon_lifecycle_through_the_cli() {
    let sandbox = Sandbox::new();
    for args in [["daemon", "status"], ["daemon", "stop"]] {
        let output = sandbox.output(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert_eq!(stderr(&output), "Error: rho daemon is not running\n");
    }
    assert!(!sandbox.socket().exists(), "status and stop never spawn");

    let mut daemon = DaemonProcess(
        sandbox
            .rho(&["daemon", "run"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until("the daemon socket", || sandbox.socket().exists());

    let status = sandbox.output(&["daemon", "status", "--json"]);
    assert!(status.status.success(), "{status:?}");
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["pid"], Value::from(daemon.0.id()));
    assert_eq!(
        status["socket"],
        Value::from(sandbox.socket().to_str().unwrap())
    );

    let second = sandbox.output(&["daemon", "run"]);
    assert_eq!(second.status.code(), Some(1), "{second:?}");
    assert_eq!(
        stderr(&second),
        format!(
            "Error: rho daemon is already running (pid {}, version {})\n",
            daemon.0.id(),
            env!("CARGO_PKG_VERSION")
        )
    );

    let stop = sandbox.output(&["daemon", "stop"]);
    assert!(stop.status.success(), "{stop:?}");
    let mut exit = None;
    wait_until("the daemon to exit", || {
        exit = daemon.0.try_wait().unwrap();
        exit.is_some()
    });
    assert!(exit.unwrap().success(), "daemon exit: {exit:?}");
    assert!(!sandbox.socket().exists(), "socket removed on stop");
}
