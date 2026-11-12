//! Installing over and removing a rho whose daemon runs (#1666), through
//! the real binary: `rho update` leaves the daemon on the binary it
//! installed without cutting a turn short, and `rho uninstall` stops the
//! daemon, and the saved `rho mcp serve --http` service, before it removes
//! anything.

use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

use pretty_assertions::assert_eq;
use rho_protocol::{methods::Status, StatusParams};

use super::sessions::Sandbox;

/// Runs `command` with `input` on its stdin and returns what it printed.
fn run(command: &mut Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A command that asks nothing may exit before reading it.
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Whether `rho daemon status` finds the sandbox's daemon running.
fn daemon_runs(sandbox: &Sandbox) -> bool {
    sandbox
        .rho("fixed")
        .args(["daemon", "status"])
        .status()
        .unwrap()
        .success()
}

// Covers: `rho uninstall --dry-run` names the running daemon in its plan and
// leaves it running; a confirmed `rho uninstall` stops it, though the
// executable is not one uninstall removes.
// Owner: CLI process (uninstall and the daemon lifecycle).
#[tokio::test]
async fn uninstall_stops_the_running_daemon() {
    let sandbox = Sandbox::new();
    let pid = sandbox
        .start_daemon("fixed")
        .await
        .request::<Status>(&StatusParams {})
        .await
        .unwrap()
        .pid;

    let preview = run(sandbox.rho("fixed").args(["uninstall", "--dry-run"]), "");
    let previewed = (
        preview.status.success(),
        stdout(&preview).contains(&format!("(pid {pid})")),
        daemon_runs(&sandbox),
    );
    let uninstall = run(sandbox.rho("fixed").arg("uninstall"), "y\n");

    assert_eq!(
        (previewed, uninstall.status.success(), daemon_runs(&sandbox)),
        ((true, true, true), true, false),
        "preview: {preview:?}\nuninstall: {uninstall:?}\ndaemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

#[cfg(unix)]
mod unix {
    use std::{
        fs,
        os::unix::fs::symlink,
        path::{Path, PathBuf},
    };

    use pretty_assertions::assert_eq;
    use rho_protocol::{methods::SessionCancel, JobState, SessionCancelParams};
    use tempfile::TempDir;

    use super::*;
    use crate::{
        sessions::settled_job,
        upgrade::{blocked_on, fifo, open_gate},
    };

    /// A directory of the tools `rho update` and `rho uninstall` run, each
    /// a stand-in (see `fixtures/daemon-install-tools.sh`), and the `PATH`
    /// that finds them before the system's.
    fn tools(dir: &Path) -> String {
        let tools = dir.join("tools");
        fs::create_dir(&tools).unwrap();
        for tool in ["cargo", "mise", "systemctl", "launchctl"] {
            symlink(
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/daemon-install-tools.sh"
                ),
                tools.join(tool),
            )
            .unwrap();
        }
        format!("{}:/usr/bin:/bin", tools.display())
    }

    /// `rho` installed at a path `rho update` installs over: at first a
    /// symlink to the binary under test.
    struct Install {
        dir: TempDir,
        path: PathBuf,
    }

    impl Install {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("rho");
            symlink(env!("CARGO_BIN_EXE_rho"), &path).unwrap();
            Self { dir, path }
        }

        /// `rho update` from the installed path, which takes the install
        /// for Cargo's and so runs the stand-in `cargo install`. The
        /// sandbox is offline, so the update goes ahead without knowing
        /// the latest release.
        fn update(&self, sandbox: &Sandbox, mode: &str) -> Output {
            run(&mut self.update_command(sandbox, mode), "")
        }

        fn update_command(&self, sandbox: &Sandbox, mode: &str) -> Command {
            let mut command = sandbox.rho_at(&self.path, mode);
            command
                .arg("update")
                .env("PATH", tools(self.dir.path()))
                .env("RHO_INSTALL_METHOD", "cargo")
                .env("RHO_TEST_INSTALL_PATH", &self.path)
                .env("RHO_TEST_RHO", env!("CARGO_BIN_EXE_rho"));
            command
        }

        /// Whether the rho that the update installed has run as the daemon.
        fn daemon_runs_the_update(&self) -> bool {
            self.dir.path().join("rho.daemon").exists()
        }
    }

    // Covers: `rho update` with an idle daemon running returns once that
    // daemon runs the binary the update installed, as the same process.
    // Owner: CLI process (update and the daemon's upgrade in place).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_restarts_an_idle_daemon_onto_the_installed_binary() {
        let sandbox = Sandbox::new();
        let install = Install::new();
        let (daemon, _client) = sandbox
            .spawn_daemon(sandbox.rho_at(&install.path, "fixed"), &[])
            .await;

        let update = install.update(&sandbox, "fixed");
        let restarted = install.daemon_runs_the_update();
        let status = sandbox
            .connect()
            .await
            .request::<Status>(&StatusParams {})
            .await
            .unwrap();

        assert_eq!(
            (
                update.status.success(),
                restarted,
                (status.pid, status.upgrade)
            ),
            (true, true, (daemon.id(), None)),
            "update: {update:?}\ndaemon log:\n{}",
            sandbox.isolated.daemon_log()
        );
    }

    // Covers: an installer that goes on after it wrote the binary, until
    // the daemon has already re-executed it, still leaves that daemon
    // running: `rho update` does not take it for one the update missed.
    // Owner: CLI process (update and the daemon's upgrade in place).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_keeps_a_daemon_that_restarted_before_the_installer_returned() {
        let sandbox = Sandbox::new();
        let install = Install::new();
        let (daemon, _client) = sandbox
            .spawn_daemon(sandbox.rho_at(&install.path, "fixed"), &[])
            .await;

        let update = run(
            install
                .update_command(&sandbox, "fixed")
                .env("RHO_TEST_INSTALL_OUTLASTS_RESTART", "1"),
            "",
        );
        // Waits while the daemon re-executes.
        let status = sandbox
            .connect()
            .await
            .request::<Status>(&StatusParams {})
            .await
            .unwrap();

        assert_eq!(
            (update.status.success(), status.pid),
            (true, daemon.id()),
            "update: {update:?}\ndaemon log:\n{}",
            sandbox.isolated.daemon_log()
        );
    }

    // Covers: `rho update` while a turn runs returns without restarting the
    // daemon or interrupting the turn, which completes on the old binary;
    // the daemon moves to the installed one afterwards.
    // Owner: CLI process (update and the daemon's upgrade in place).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_leaves_a_running_turn_to_finish_before_the_daemon_restarts() {
        let sandbox = Sandbox::new();
        sandbox.set_permission_mode("bypass");
        let install = Install::new();
        let (_daemon, client) = sandbox
            .spawn_daemon(sandbox.rho_at(&install.path, "tool-call"), &[])
            .await;
        let session = sandbox.new_session(&client).await;
        let release = fifo(sandbox.workspace.join("release.fifo"));
        sandbox
            .prompt_tool_call(&client, &session, &blocked_on("release.fifo"))
            .await;

        let update = install.update(&sandbox, "tool-call");
        let restarted_during_the_turn = install.daemon_runs_the_update();
        open_gate(&sandbox, &release).await;
        let turn = settled_job(&sandbox, &client, &session).await.state;
        sandbox
            .poll_until("the daemon to run the update", async || {
                install.daemon_runs_the_update().then_some(())
            })
            .await;

        assert_eq!(
            (update.status.success(), restarted_during_the_turn, turn),
            (true, false, JobState::Completed),
            "update: {update:?}\ndaemon log:\n{}",
            sandbox.isolated.daemon_log()
        );
    }

    /// `rho` as mise installs it: in a directory of its version's own, which
    /// an update leaves as it is.
    struct MiseInstall {
        root: TempDir,
        installs: PathBuf,
        version: PathBuf,
        rho: PathBuf,
    }

    impl MiseInstall {
        fn new() -> Self {
            // A hard link, which needs the built executable's filesystem:
            // rho resolves a symlink to its target, outside mise's directory.
            let root =
                TempDir::new_in(Path::new(env!("CARGO_BIN_EXE_rho")).parent().unwrap()).unwrap();
            let installs = root.path().canonicalize().unwrap().join("installs");
            let version = installs.join("rho/1.0.0");
            let rho = version.join("bin/rho");
            fs::create_dir_all(rho.parent().unwrap()).unwrap();
            fs::hard_link(env!("CARGO_BIN_EXE_rho"), &rho).unwrap();
            Self {
                root,
                installs,
                version,
                rho,
            }
        }

        /// `rho update` from the mise install, which runs the stand-in
        /// `mise upgrade`.
        fn update(&self, sandbox: &Sandbox, mode: &str) -> Output {
            run(
                sandbox
                    .rho_at(&self.rho, mode)
                    .arg("update")
                    .env("PATH", tools(self.root.path()))
                    .env("MISE_INSTALLS_DIR", &self.installs)
                    .env("RHO_TEST_MISE_INSTALL", &self.version),
                "",
            )
        }
    }

    // Covers: an update that installs the new version in a directory of its
    // own, as mise does, leaves nothing at the daemon's path to re-execute:
    // `rho update` stops the idle daemon, so the next client starts the
    // updated rho.
    // Owner: CLI process (update and the daemon lifecycle).
    #[tokio::test]
    async fn update_stops_an_idle_daemon_that_cannot_reach_the_installed_binary() {
        let sandbox = Sandbox::new();
        let install = MiseInstall::new();
        let (_daemon, client) = sandbox
            .spawn_daemon(sandbox.rho_at(&install.rho, "fixed"), &[])
            .await;
        drop(client);

        let update = install.update(&sandbox, "fixed");

        assert_eq!(
            (update.status.success(), daemon_runs(&sandbox)),
            (true, false),
            "update: {update:?}\ndaemon log:\n{}",
            sandbox.isolated.daemon_log()
        );
    }

    // Covers: that update leaves a daemon running whose only work is a
    // background process a session's cancelled turn left behind, which a
    // stop would kill though no job is in progress.
    // Owner: CLI process (update and the daemon lifecycle).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_leaves_a_daemon_with_a_background_process_running() {
        let sandbox = Sandbox::new();
        sandbox.set_permission_mode("bypass");
        let install = MiseInstall::new();
        let (_daemon, client) = sandbox
            .spawn_daemon(sandbox.rho_at(&install.rho, "tool-call"), &[])
            .await;
        let session = sandbox.new_session(&client).await;
        let release = fifo(sandbox.workspace.join("background.fifo"));
        let background = serde_json::json!({
            "id": "background",
            "name": "process",
            "arguments": {
                "action": "start",
                "command": "touch started && read _ < background.fifo",
            },
        });
        sandbox
            .prompt_tool_call(&client, &session, &background)
            .await;
        sandbox
            .poll_until("the background process to start", async || {
                sandbox.workspace.join("started").exists().then_some(())
            })
            .await;
        // A turn waits for the processes it started, so only its cancel
        // lets one outlive it.
        client
            .request::<SessionCancel>(&SessionCancelParams {
                session: session.clone(),
            })
            .await
            .unwrap();
        let turn = settled_job(&sandbox, &client, &session).await.state;

        let update = install.update(&sandbox, "tool-call");
        let kept = daemon_runs(&sandbox);
        open_gate(&sandbox, &release).await;

        assert_eq!(
            (turn, update.status.success(), kept),
            (JobState::Cancelled, true, true),
            "update: {update:?}\ndaemon log:\n{}",
            sandbox.isolated.daemon_log()
        );
    }

    // Covers: with the documented systemd unit, its enabling link, and the
    // LaunchAgent saved, `rho uninstall --dry-run` lists each file and asks
    // no service manager; a confirmed uninstall has each manager stop its
    // service, deletes the files, and stops the daemon.
    // Owner: CLI process (uninstall, service files, and the daemon).
    #[tokio::test]
    async fn uninstall_stops_and_removes_the_saved_service() {
        let sandbox = Sandbox::new();
        let _client = sandbox.start_daemon("fixed").await;
        let home = sandbox.isolated.home.canonicalize().unwrap();
        let units = home.join(".config/systemd/user");
        let wants = units.join("default.target.wants");
        let agents = home.join("Library/LaunchAgents");
        fs::create_dir_all(&wants).unwrap();
        fs::create_dir_all(&agents).unwrap();
        let unit = units.join("rho-mcp.service");
        let link = wants.join("rho-mcp.service");
        let agent = agents.join("dev.rho.mcp.plist");
        fs::write(&unit, "[Service]\n").unwrap();
        symlink(&unit, &link).unwrap();
        fs::write(&agent, "").unwrap();
        let files = [&link, &unit, &agent];
        let saved = || files.map(|file| file.symlink_metadata().is_ok());
        let log = home.join("tools.log");
        let path = tools(&home);
        let uninstall = |args: &[&str], input: &str| {
            run(
                sandbox
                    .rho("fixed")
                    .arg("uninstall")
                    .args(args)
                    .env("PATH", &path)
                    .env("RHO_TEST_TOOL_LOG", &log),
                input,
            )
        };

        let preview = uninstall(&["--dry-run"], "");
        let listed = files.map(|file| stdout(&preview).contains(&format!("{file:?}")));
        let previewed = (listed, saved(), log.exists(), daemon_runs(&sandbox));
        let removed = uninstall(&[], "y\n");
        // SAFETY: geteuid has no arguments or memory-safety preconditions.
        let user = unsafe { libc::geteuid() };

        assert_eq!(
            (
                previewed,
                removed.status.success(),
                saved(),
                fs::read_to_string(&log).unwrap_or_default(),
                daemon_runs(&sandbox),
            ),
            (
                ([true; 3], [true; 3], false, true),
                true,
                [false; 3],
                format!(
                    "systemctl --user disable --now rho-mcp.service\nlaunchctl bootout gui/{user}/dev.rho.mcp\n"
                ),
                false,
            ),
            "preview: {preview:?}\nuninstall: {removed:?}"
        );
    }
}

// Covers: where a running daemon keeps an installer from replacing the rho
// executable, `rho update` stops an idle daemon before it shows the update
// command, so that command can install.
// Owner: CLI process (update and the daemon lifecycle).
#[cfg(windows)]
#[tokio::test]
async fn update_stops_an_idle_daemon_that_holds_the_executable() {
    let sandbox = Sandbox::new();
    let _client = sandbox.start_daemon("fixed").await;

    let update = run(
        sandbox
            .rho("fixed")
            .arg("update")
            .env("RHO_INSTALL_METHOD", "cargo"),
        "",
    );

    assert_eq!(
        (update.status.success(), daemon_runs(&sandbox)),
        (true, false),
        "update: {update:?}\ndaemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
