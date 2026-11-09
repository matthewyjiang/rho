//! Environment hygiene and isolated Rho launch plans.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
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
/// # Teardown and drop order
///
/// Dropping the sandbox stops its daemon (SIGTERM, then SIGKILL; see
/// `daemon.rs`) before it deletes the directory, also while a test panics.
/// So a process that may start a daemon, such as a TUI or `rho acp`, must
/// be dropped or killed before the sandbox: one that outlives the teardown
/// can start a new daemon in a home that is about to be deleted. Declare
/// such a process before the sandbox in a struct (fields drop in order),
/// or after it in a function (locals drop in reverse).
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
        let run = temp.path().join("run");
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(&run)?;
        #[cfg(not(unix))]
        fs::create_dir(&run)?;
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

    /// The whole environment of a rho process outside a PTY, the daemon
    /// and its clients: [`default_clean_env`], then [`Self::env`], then the
    /// file credential store and backtraces (later pairs win).
    ///
    /// `RHO_CREDENTIAL_STORE` is set here rather than in [`Self::env`]:
    /// PTY launches leave the store to `config.toml`, so a first-run
    /// scenario can remove it there and reach the store prompt.
    pub fn command_env(&self) -> Vec<(String, String)> {
        let mut env = default_clean_env();
        env.extend(self.env());
        // Fixture sessions must not query the developer's OS keyring.
        env.push(("RHO_CREDENTIAL_STORE".into(), "file".into()));
        // A daemon's panic then logs where it happened.
        env.push(("RUST_BACKTRACE".into(), "1".into()));
        env
    }

    /// `binary` in the workspace with exactly [`Self::command_env`]. rho
    /// finds the sandbox's `config.toml` through `RHO_HOME`, so it needs
    /// no `--config`, which `rho login` and `rho run --detach` refuse.
    pub fn command(&self, binary: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(binary);
        command
            .env_clear()
            .envs(self.command_env())
            .current_dir(&self.workspace);
        command
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        // rho runs no daemon off Unix.
        #[cfg(unix)]
        crate::daemon::stop(self);
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
    /// Build a matrix-mode launch plan in `home`'s sandbox. rho reads the
    /// sandbox's `config.toml` through `RHO_HOME`, so the plan passes no
    /// `--config`.
    pub fn matrix(binary: impl Into<PathBuf>, home: &IsolatedHome, size: PtySize) -> Self {
        let mut env = default_clean_env();
        env.extend(home.env());
        env.push(("RHO_TUI_TEST_MODE".into(), "matrix".into()));
        // Where fixture release markers are, for a fixture the daemon hosts.
        env.push((
            "RHO_TUI_FIXTURE_WORKSPACE".into(),
            home.workspace.display().to_string(),
        ));
        // A crash then says where.
        env.push(("RUST_BACKTRACE".into(), "1".into()));
        Self {
            binary: binary.into(),
            size,
            args: Vec::new(),
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
