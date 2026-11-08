//! Environment hygiene and isolated Rho launch plans.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use tempfile::TempDir;

use crate::pty::PtySize;

/// Proxy that refuses every connection (TCP discard port on loopback).
const OFFLINE_PROXY: &str = "http://127.0.0.1:9";

/// Host terminal identity markers that can leak into child behavior.
pub const HOST_TERMINAL_MARKERS: &[&str] = &[
    "CURSOR_TRACE_ID",
    "VSCODE_GIT_ASKPASS_MAIN",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERMINAL_EMULATOR",
    "WEZTERM_VERSION",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "ITERM_PROFILE",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "TERM_SESSION_ID",
    "KITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "TERMINATOR_UUID",
    "VTE_VERSION",
    "WT_SESSION",
    "TMUX",
    "TMUX_PANE",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "STY",
    "BYOBU_BACKEND",
    "BYOBU_CONFIG_DIR",
    "NVIM",
    "NVIM_LISTEN_ADDRESS",
    "VIM_TERMINAL",
    "INSIDE_EMACS",
    "HERDR_ENV",
    "HERDR_SOCKET_PATH",
    "HERDR_PANE_ID",
];

/// Optional host profile reinjection for deliberate profile tests.
#[derive(Clone, Debug, Default)]
pub struct HostProfile {
    pub vars: BTreeMap<String, String>,
}

/// How long [`IsolatedHome::stop_daemon`] waits for the daemon to exit after
/// SIGTERM, and again after SIGKILL.
//
// Tripwire, not a tuning knob. SIGTERM is the daemon's graceful stop: rho
// measured that exit at max 44 ms with all 24 cores saturated
// (`DAEMON_STOP_TIMEOUT` in `app::daemon_cli`), plus up to `DRAIN_TIMEOUT`
// (4 s, `daemon_host::sessions`) while running turns save. 10 s covers both
// on a loaded CI host running many sandboxes at once; hitting it means
// shutdown is broken, and SIGKILL still keeps the daemon from outliving the
// test.
const DAEMON_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between checks while [`IsolatedHome::stop_daemon`] waits. Each
/// is a `flock` probe or a `waitpid` and a `kill(pid, 0)`; nothing notifies
/// a process that is not the daemon's parent when it exits.
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Temporary HOME, rho home, and runtime directory that keep a test's rho
/// processes, its daemon included, off the developer's state. Safe to use
/// from parallel tests: nothing is shared between two sandboxes.
///
/// Under the temporary root: `home/` (`HOME`), `home/.rho/` (`RHO_HOME`,
/// holding `config.toml`), `workspace/`, `tmp/` (`TMPDIR`), and `run/`
/// (`XDG_RUNTIME_DIR`, mode 0700 on Unix). [`Self::env`] points a process
/// at them; [`Self::command`] and [`RhoLaunchPlan::matrix`] build on it.
///
/// # One sandbox, one daemon, one fixture mode
///
/// Every rho process in the sandbox reaches the same daemon, at
/// [`Self::daemon_socket`]. A client that finds none running starts one,
/// which inherits that client's whole environment, fixture variables
/// included (`RHO_AUTOMATION_TEST_*`, `RHO_TUI_TEST_MODE`), and runs until
/// it is stopped. So a sandbox runs one fixture mode, which every client in
/// it must set: a client in another mode would be served by a daemon frozen
/// in the first one's.
///
/// The socket path is the root's plus 37 bytes (`/run/rho/<16 hex>/daemon.sock`);
/// rho refuses one over the Unix limit (107 bytes on Linux, 103 on macOS)
/// with `SocketPathTooLong`, so a `TMPDIR` whose temporary directories are
/// longer than 66 bytes (70 on Linux) fails loud rather than escaping the
/// sandbox.
///
/// Dropping the sandbox stops its daemon ([`Self::stop_daemon`]) before it
/// deletes the directory, also while a test panics.
pub struct IsolatedHome {
    pub home: PathBuf,
    pub workspace: PathBuf,
    pub config_path: PathBuf,
    /// Deleted once [`Drop`] stopped the daemon running from it: a struct's
    /// fields drop after its `drop` returns.
    temp: TempDir,
}

impl IsolatedHome {
    pub fn new() -> Result<Self> {
        let temp = TempDir::new().context("failed to create isolated HOME")?;
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        let rho_dir = home.join(".rho");
        fs::create_dir_all(&rho_dir)?;
        fs::create_dir_all(&workspace)?;
        fs::create_dir_all(temp.path().join("tmp"))?;
        // Private, as a real XDG_RUNTIME_DIR is.
        let mut run = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut run, 0o700);
        run.create(temp.path().join("run"))?;
        let config_path = rho_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"

[behavior]
# Keep matrix runs on the isolated file store so /login never prompts and
# never touches the developer OS keyring.
credential_store = "file"
"#,
        )
        .context("failed to write isolated config.toml")?;
        Ok(Self {
            home,
            workspace,
            config_path,
            temp,
        })
    }

    pub fn path(&self) -> &Path {
        self.temp.path()
    }

    /// `RHO_HOME`: `home/.rho`, where rho looks without it too.
    pub fn rho_home(&self) -> PathBuf {
        self.home.join(".rho")
    }

    /// The variables that put a process in this sandbox, layered over
    /// [`default_clean_env`] (later pairs win).
    pub fn env(&self) -> Vec<(String, String)> {
        let text = |path: PathBuf| path.display().to_string();
        let mut env = vec![
            ("HOME".into(), text(self.home.clone())),
            ("RHO_HOME".into(), text(self.rho_home())),
            // Child temp files (installer logs, recovery drafts) die with the
            // sandbox.
            ("TMPDIR".into(), text(self.path().join("tmp"))),
            // On every OS, so the daemon's socket lands in the sandbox (see
            // the type's docs). On Linux it also isolates the session bus,
            // which HOME alone does not: credential-store probes could reach
            // the developer's keyring and block. Removing it instead would
            // let D-Bus clients fall back to the user's default bus.
            ("XDG_RUNTIME_DIR".into(), text(self.path().join("run"))),
        ];
        // Windows home/config discovery must not fall back to the real profile.
        #[cfg(windows)]
        for (key, dir) in [
            ("USERPROFILE", self.home.clone()),
            ("APPDATA", self.home.join("AppData/Roaming")),
            ("LOCALAPPDATA", self.home.join("AppData/Local")),
        ] {
            env.push((key.into(), text(dir)));
        }
        // Keep the child off the internet. Without this every launch downloads
        // and parses the models.dev catalog in the background: CPU that
        // dominates debug PTY runs, and a hydrate whose arrival time can
        // rewrite the first system prompt. The discard port refuses at once;
        // loopback stays direct for mock servers. A test that needs a
        // different proxy layers it on top (later pairs win).
        for key in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
            env.push((key.into(), OFFLINE_PROXY.into()));
        }
        env.push(("NO_PROXY".into(), "localhost,127.0.0.1,::1".into()));
        env
    }

    /// `binary` in the workspace with exactly the sandbox's environment
    /// ([`default_clean_env`] plus [`Self::env`]) and the file credential
    /// store, for rho processes outside a PTY: the daemon and its clients.
    ///
    /// `RHO_CREDENTIAL_STORE` is set here rather than in [`Self::env`]:
    /// PTY launches leave the store to `config.toml`, so a first-run
    /// scenario can remove it there and reach the store prompt.
    pub fn command(&self, binary: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(binary);
        command
            .env_clear()
            .envs(default_clean_env())
            .envs(self.env())
            // Fixture sessions must not query the developer's OS keyring.
            .env("RHO_CREDENTIAL_STORE", "file")
            .current_dir(&self.workspace);
        command
    }

    /// The daemon's log so far, empty before it starts.
    pub fn daemon_log(&self) -> String {
        fs::read_to_string(self.rho_home().join("logs/daemon.log")).unwrap_or_default()
    }

    /// The socket the sandbox's daemon listens on, once one does. It is
    /// `run/rho/<id>/daemon.sock`, and the sandbox's one rho home has one
    /// id, which rho derives from the home's canonical path.
    pub fn daemon_socket(&self) -> Option<PathBuf> {
        fs::read_dir(self.path().join("run/rho"))
            .ok()?
            .flatten()
            .map(|runtime_dir| runtime_dir.path().join("daemon.sock"))
            .find(|socket| socket.exists())
    }

    /// The pid in the daemon's record (`daemon/daemon.json` in the rho
    /// home), which a daemon writes once its socket is bound. A daemon
    /// that was killed leaves its record behind.
    pub fn daemon_pid(&self) -> Option<u32> {
        #[derive(serde::Deserialize)]
        struct DaemonRecord {
            pid: u32,
        }
        let record = fs::read(self.rho_home().join("daemon/daemon.json")).ok()?;
        serde_json::from_slice::<DaemonRecord>(&record)
            .ok()
            .map(|record| record.pid)
    }

    /// Stops the daemon running from this sandbox, if any: SIGTERM, then
    /// SIGKILL if it outlives `DAEMON_TEARDOWN_TIMEOUT`. SIGTERM comes first
    /// because the daemon's graceful stop also ends the tool processes its
    /// sessions started, which SIGKILL would orphan. A daemon this process
    /// started is reaped, so it is gone, not a zombie, when this returns.
    ///
    /// Panics, unless the thread is already panicking, if the daemon
    /// survives both signals.
    #[cfg(unix)]
    pub fn stop_daemon(&self) {
        let Some(pid) = self.running_daemon_pid() else {
            return;
        };
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            // SAFETY: `kill` takes no pointers and has no memory-safety
            // preconditions.
            unsafe { libc::kill(pid, signal) };
            if exits_within(pid, DAEMON_TEARDOWN_TIMEOUT) {
                return;
            }
        }
        fail_teardown(&format!(
            "rho daemon (pid {pid}) outlived SIGTERM and SIGKILL, waiting DAEMON_TEARDOWN_TIMEOUT ({DAEMON_TEARDOWN_TIMEOUT:?}) after each"
        ));
    }

    /// rho runs no daemon off Unix.
    #[cfg(not(unix))]
    pub fn stop_daemon(&self) {}

    /// The pid of the daemon that holds the sandbox's daemon lock, waiting
    /// out one that took the lock but has not written its record yet.
    /// `None` when no daemon holds the lock: a record a killed daemon left
    /// names a pid another process may have since been given.
    #[cfg(unix)]
    fn running_daemon_pid(&self) -> Option<libc::pid_t> {
        let lock = self.rho_home().join("daemon/daemon.lock");
        let started = Instant::now();
        loop {
            if !daemon_lock_held(&lock) {
                return None;
            }
            if let Some(pid) = self.daemon_pid() {
                return libc::pid_t::try_from(pid).ok();
            }
            if started.elapsed() >= DAEMON_TEARDOWN_TIMEOUT {
                fail_teardown(&format!(
                    "rho daemon held {} for DAEMON_TEARDOWN_TIMEOUT ({DAEMON_TEARDOWN_TIMEOUT:?}) without writing its record",
                    lock.display()
                ));
                return None;
            }
            std::thread::sleep(DAEMON_POLL_INTERVAL);
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        self.stop_daemon();
    }
}

/// Whether a daemon holds `lock`, probed as rho's own `daemon::is_held`
/// does: with a non-blocking shared `flock` that the daemon's exclusive one
/// refuses.
#[cfg(unix)]
fn daemon_lock_held(lock: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = fs::File::open(lock) else {
        return false;
    };
    // SAFETY: `flock` takes only a descriptor, which `file` keeps open.
    let probed = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    probed != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

/// Whether `pid` exits within `limit`. A pid that is this process's child
/// (a daemon a test ran itself) is reaped here: unreaped, it would linger
/// as a zombie, which `kill(pid, 0)` still finds.
#[cfg(unix)]
fn exits_within(pid: libc::pid_t, limit: Duration) -> bool {
    let started = Instant::now();
    loop {
        // SAFETY: a null status pointer is allowed, and WNOHANG never blocks.
        let reaped = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) } == pid;
        // SAFETY: signal 0 only checks that `pid` exists and is ours to signal.
        if reaped || unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        if started.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(DAEMON_POLL_INTERVAL);
    }
}

/// Fails a sandbox teardown loudly, without turning a test that is already
/// panicking into an abort.
#[cfg(unix)]
fn fail_teardown(message: &str) {
    if std::thread::panicking() {
        eprintln!("{message}");
    } else {
        panic!("{message}");
    }
}

/// Launch plan for a Rho binary under the fixture matrix.
#[derive(Clone, Debug)]
pub struct RhoLaunchPlan {
    pub binary: PathBuf,
    pub size: PtySize,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

impl RhoLaunchPlan {
    /// Build a matrix-mode launch plan in `home`'s sandbox, with its config.
    pub fn matrix(binary: impl Into<PathBuf>, home: &IsolatedHome, size: PtySize) -> Self {
        let mut env = default_clean_env();
        env.extend(home.env());
        env.push(("RHO_TUI_TEST_MODE".into(), "matrix".into()));
        // A crash then says where.
        env.push(("RUST_BACKTRACE".into(), "1".into()));
        Self {
            binary: binary.into(),
            size,
            args: vec!["--config".into(), home.config_path.display().to_string()],
            env,
            cwd: home.workspace.clone(),
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn with_host_profile(mut self, profile: &HostProfile) -> Self {
        for (key, value) in &profile.vars {
            self.env.push((key.clone(), value.clone()));
        }
        self
    }
}

/// Baseline env pairs that form the complete child environment for isolated runs.
pub fn default_clean_env() -> Vec<(String, String)> {
    let mut env = vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("LANG".into(), "C.UTF-8".into()),
        ("LC_ALL".into(), "C.UTF-8".into()),
    ];
    // Preserve a minimal PATH so the child can resolve dynamic loaders and helpers.
    if let Ok(path) = env::var("PATH") {
        env.push(("PATH".into(), path));
    }
    // Runtime/loader and temp dirs are not credentials; keep them so binaries start.
    for key in [
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "TMPDIR",
        "TMP",
        "TEMP",
        "USER",
        "LOGNAME",
        "SHELL",
        "XDG_RUNTIME_DIR",
        // Windows loader, system tools, and executable lookup essentials. Do
        // not inherit USERPROFILE or APPDATA: IsolatedHome::env supplies
        // isolated paths.
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
    ] {
        if let Ok(value) = env::var(key) {
            env.push((key.into(), value));
        }
    }
    env
}

/// Resolve the Rho binary for harness runs.
///
/// Order:
/// 1. `RHO_PTY_BIN`
/// 2. `CARGO_BIN_EXE_rho`
/// 3. workspace `target/{debug,release}/rho` discovered from this crate or cwd
pub fn resolve_rho_binary() -> Result<PathBuf> {
    if let Ok(path) = env::var("RHO_PTY_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!("RHO_PTY_BIN does not point to a file: {}", path.display());
    }
    if let Ok(path) = env::var("CARGO_BIN_EXE_rho") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    // Only debug binaries are discovered implicitly: matrix fixtures are
    // compiled under cfg(debug_assertions), so a release binary silently runs
    // without them and every scenario fails as "not signed in". A release
    // binary is still usable through an explicit RHO_PTY_BIN.
    let mut candidates = Vec::new();
    let mut release_fallbacks = Vec::new();
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let manifest = PathBuf::from(manifest_dir);
        // crates/rho-tui-pty -> workspace root
        if let Some(workspace) = manifest.parent().and_then(Path::parent) {
            candidates.push(workspace.join("target/debug/rho"));
            release_fallbacks.push(workspace.join("target/release/rho"));
        }
    }
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("target/debug/rho"));
        release_fallbacks.push(cwd.join("target/release/rho"));
    }

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    if let Some(release) = release_fallbacks.iter().find(|path| path.is_file()) {
        bail!(
            "found only a release binary at {}; matrix fixtures are compiled with \
             debug_assertions, so scenarios would fail as \"not signed in\". Build a \
             debug binary (`cargo build -p rho-coding-agent`) or point RHO_PTY_BIN at \
             a binary you intend to run without fixtures.",
            release.display()
        );
    }

    // Last resort: ask cargo for the bin path.
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(target_dir) = value.get("target_directory").and_then(|v| v.as_str()) {
                    let debug = PathBuf::from(target_dir).join("debug/rho");
                    if debug.is_file() {
                        return Ok(debug);
                    }
                    let release = PathBuf::from(target_dir).join("release/rho");
                    if release.is_file() {
                        return Ok(release);
                    }
                }
            }
        }
    }

    bail!(
        "could not resolve rho binary; set RHO_PTY_BIN or build with `cargo build -p rho-coding-agent`"
    )
}
