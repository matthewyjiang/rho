//! Environment hygiene and isolated Rho launch plans.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use tempfile::TempDir;

use crate::pty::PtySize;

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

/// Temporary HOME and config root that keep PTY runs off the developer's state.
pub struct IsolatedHome {
    _temp: TempDir,
    pub home: PathBuf,
    pub workspace: PathBuf,
    pub config_path: PathBuf,
}

impl IsolatedHome {
    pub fn new() -> Result<Self> {
        let temp = TempDir::new().context("failed to create isolated HOME")?;
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        let rho_dir = home.join(".rho");
        fs::create_dir_all(&rho_dir)?;
        fs::create_dir_all(&workspace)?;
        let config_path = rho_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"
"#,
        )
        .context("failed to write isolated config.toml")?;
        Ok(Self {
            _temp: temp,
            home,
            workspace,
            config_path,
        })
    }

    pub fn path(&self) -> &Path {
        self._temp.path()
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
    /// Build a matrix-mode launch plan with isolated HOME and config.
    pub fn matrix(binary: impl Into<PathBuf>, home: &IsolatedHome, size: PtySize) -> Self {
        let mut env = default_clean_env();
        env.push(("HOME".into(), home.home.display().to_string()));
        env.push(("RHO_TUI_TEST_MODE".into(), "matrix".into()));
        // Keep keyring/credential side effects out of the developer account.
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

/// Baseline env pairs applied after host-marker stripping.
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
    if let Ok(lib) = env::var("LD_LIBRARY_PATH") {
        env.push(("LD_LIBRARY_PATH".into(), lib));
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

    let mut candidates = Vec::new();
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let manifest = PathBuf::from(manifest_dir);
        // crates/rho-tui-pty -> workspace root
        if let Some(workspace) = manifest.parent().and_then(Path::parent) {
            candidates.push(workspace.join("target/debug/rho"));
            candidates.push(workspace.join("target/release/rho"));
        }
        // crates/rho -> workspace root
        if let Some(workspace) = manifest.parent().and_then(Path::parent) {
            candidates.push(workspace.join("target/debug/rho"));
        }
    }
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("target/debug/rho"));
        candidates.push(cwd.join("target/release/rho"));
    }

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_home_writes_config() {
        let home = IsolatedHome::new().unwrap();
        let config = fs::read_to_string(&home.config_path).unwrap();
        assert!(config.contains("check_for_updates = false"));
        assert!(home.workspace.is_dir());
    }

    #[test]
    fn host_markers_include_tmux_and_herdr() {
        assert!(HOST_TERMINAL_MARKERS.contains(&"TMUX"));
        assert!(HOST_TERMINAL_MARKERS.contains(&"HERDR_ENV"));
    }
}
