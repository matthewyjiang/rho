//! A sandboxed rho home for driving `rho daemon` through the real binary.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use tempfile::TempDir;

/// How often a wait polls the daemon's socket, process, or job list. The
/// daemon pushes nothing to clients yet (#1657), so polling is the only way
/// to see it change.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A temporary directory holding the rho home (`<root>/home`). With no
/// `XDG_RUNTIME_DIR`, the daemon's socket is `<home>/run/daemon.sock`; where
/// it goes under `XDG_RUNTIME_DIR` is the path unit tests' job.
pub(crate) struct DaemonSandbox {
    _dir: TempDir,
    /// The temporary directory's canonical path. `rho daemon` canonicalizes
    /// the home, so the paths it reports are under this one (on macOS the
    /// temporary directory is under the `/var` -> `/private/var` symlink).
    root: PathBuf,
}

impl DaemonSandbox {
    pub(crate) fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Self { _dir: dir, root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    pub(crate) fn socket(&self) -> PathBuf {
        self.home().join("run/daemon.sock")
    }

    pub(crate) fn daemon_log(&self) -> String {
        std::fs::read_to_string(self.home().join("logs/daemon.log")).unwrap_or_default()
    }

    /// `rho` with its data root and socket inside the sandbox and
    /// its daemon logging to the file [`Self::daemon_log`] reads. Only the
    /// child's environment changes.
    pub(crate) fn rho(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
        command
            .env("HOME", self.root())
            .env("RHO_HOME", self.home())
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("JOURNAL_STREAM")
            .env_remove("RHO_LOG")
            .stdin(Stdio::null());
        command
    }

    /// The environment a client in the sandbox sends, which its sessions'
    /// processes run with (#1659): this process's, changed as [`Self::rho`]
    /// changes a child's, so nothing a session runs reaches the developer's
    /// rho home or daemon.
    pub(crate) fn client_environment(&self) -> rho_protocol::ClientEnvironment {
        let mut variables: std::collections::BTreeMap<String, String> =
            rho_protocol::ClientEnvironment::of_this_process()
                .into_iter()
                .collect();
        for (name, value) in self.rho().get_envs() {
            let name = name.to_str().unwrap().to_owned();
            match value {
                Some(value) => variables.insert(name, value.to_str().unwrap().to_owned()),
                None => variables.remove(&name),
            };
        }
        variables.into_iter().collect()
    }

    /// Calls `attempt` every [`POLL_INTERVAL`] until it returns `Some`, and
    /// fails with the daemon log once `deadline` passes.
    pub(crate) async fn poll_until<T>(
        &self,
        what: &str,
        deadline: Duration,
        mut attempt: impl AsyncFnMut() -> Option<T>,
    ) -> T {
        let started = Instant::now();
        loop {
            if let Some(value) = attempt().await {
                return value;
            }
            assert!(
                started.elapsed() < deadline,
                "timed out after {deadline:?} waiting for {what}; daemon log:\n{}",
                self.daemon_log()
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Kills the daemon if the test ends without stopping it.
pub(crate) struct DaemonProcess(pub(crate) Child);

impl DaemonProcess {
    pub(crate) fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        self.kill();
    }
}
