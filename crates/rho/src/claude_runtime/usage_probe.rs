//! Read Claude Code `/usage` through a dedicated PTY session.
//!
//! Claude owns the subscription token. Rho never reads credential files; it
//! spawns the `claude` TUI, sends `/usage`, and parses the panel.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use thiserror::Error;

use super::{
    auth::{self, ClaudeAuthError},
    executable,
    rate_limit::{self, RateLimitState},
    usage_parse::{named_window_keys, parse_usage_screen},
};

#[cfg(unix)]
#[path = "usage_probe_drive.rs"]
mod drive;

/// A full TUI start is ~10–20s. Reuse a successful probe for the rest of a
/// work burst so `/limits` does not pay that again. Claude's own last-known
/// `/usage` window is 60 minutes; five minutes stays live without stacking
/// Claude processes.
pub(crate) const LIVE_TTL: Duration = Duration::from_secs(5 * 60);

/// Time budgets for one `/usage` read after the idle prompt. Tests shrink
/// these so hung-child cases do not wait out the production values.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProbeBudget {
    /// `/usage` then Anthropic's usage endpoint. Warm captures paint "% used"
    /// in under 1s, but a cold start stacks startup latency with a slow usage
    /// refresh; a user-visible probe failure landed 20-30s after `/limits`,
    /// which is exactly startup + the old 15s budget. Warm runs never wait
    /// this long; it only bounds a hung refresh.
    pub(crate) panel_wait: Duration,
    /// Wait this long only while the screen names a window we have not parsed.
    pub(crate) grow: Duration,
}

const PROBE_BUDGET: ProbeBudget = ProbeBudget {
    panel_wait: Duration::from_secs(30),
    grow: Duration::from_secs(2),
};

const PROMPT_MARKERS: &[&str] = &["? for shortcuts", "try \"", "shift+tab to cycle"];
const TRUST_MARKERS: &[&str] = &["trust this folder", "do you trust"];
const LOGIN_MARKERS: &[&str] = &["log in", "sign in to"];
const PANEL_MARKERS: &[&str] = &["Current session", "% used", "%used"];
/// Claude can keep percentages visible after a failed refresh, and hides the
/// spinner when showing these notices. None of those windows are live results.
const REFRESH_FAILURE_MARKERS: &[&str] = &[
    "failed to load usage",
    "showing last-known usage",
    "could not refresh usage",
    "partial usage data",
    "per-model breakdown unavailable",
    "usage endpoint is rate limited",
];

#[derive(Debug, Error)]
pub(crate) enum UsageProbeError {
    #[error("claude code: binary not found on PATH")]
    BinaryMissing,
    #[error("claude code: not signed in - run /login claude-code")]
    NotSignedIn,
    #[error("claude code: usage probe needs a Unix PTY")]
    Unsupported,
    #[error("claude code: could not start usage probe: {0}")]
    Spawn(String),
    #[error("claude code: usage probe cancelled")]
    Cancelled,
    #[error("claude code: timed out waiting for {what}: {screen}")]
    TimeoutScreen { what: &'static str, screen: String },
    #[error("claude code: claude exited before {what}: {screen}")]
    Exited { what: &'static str, screen: String },
    #[error("claude code: /usage refresh failed: {screen}")]
    RefreshFailed { screen: String },
    #[error("claude code: /usage panel was not readable")]
    Unparseable,
    #[error("claude code: auth preflight failed: {0}")]
    Auth(#[from] ClaudeAuthError),
}

/// Probe finished without a live panel. `/limits` should keep disk windows.
#[derive(Debug)]
pub(crate) enum UsageProbeOutcome {
    Ready(RateLimitState),
    Unavailable,
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Unix PTY is required to drive the interactive Claude TUI.
pub(crate) fn probe_supported() -> bool {
    cfg!(unix)
}

/// Auth preflight, then a blocking PTY `/usage` read.
pub(crate) async fn fetch_usage() -> Result<UsageProbeOutcome, UsageProbeError> {
    match auth::query().await {
        Ok(status) if status.logged_in => {}
        Ok(_) => return Ok(UsageProbeOutcome::Unavailable),
        Err(ClaudeAuthError::BinaryMissing) => return Ok(UsageProbeOutcome::Unavailable),
        Err(error) => return Err(error.into()),
    }
    if !probe_supported() {
        return Err(UsageProbeError::Unsupported);
    }
    let abort = Arc::new(AtomicBool::new(false));
    let _cancel = CancelOnDrop(Arc::clone(&abort));
    let mut state = tokio::task::spawn_blocking(move || probe_usage_blocking(&abort))
        .await
        .map_err(|error| UsageProbeError::Spawn(error.to_string()))??;
    let now = rate_limit::now_unix();
    state.last_probe_unix = Some(now);
    // Capture stamps parse time per window. Restamp seconds so `/limits` age
    // is the probe instant, not a mid-panel parse that crossed a second.
    for window in &mut state.windows {
        window.observed_at_unix = now;
    }
    persist_probe_state(state)
}

fn persist_probe_state(state: RateLimitState) -> Result<UsageProbeOutcome, UsageProbeError> {
    let path = match rate_limit::default_state_path() {
        Ok(path) => path,
        Err(error) => {
            tracing::debug!(
                error = %error,
                "claude rate-limit cache path unavailable; returning unpersisted probe"
            );
            return Ok(UsageProbeOutcome::Ready(state));
        }
    };
    match rate_limit::store_state(&path, state.clone()) {
        Ok(merged) => Ok(UsageProbeOutcome::Ready(merged)),
        Err(error) => {
            tracing::warn!(error = %error, "failed to persist claude rate-limit cache");
            Ok(UsageProbeOutcome::Ready(state))
        }
    }
}

fn probe_usage_blocking(abort: &AtomicBool) -> Result<RateLimitState, UsageProbeError> {
    let executable = executable::resolve().map_err(|error| match error {
        ClaudeAuthError::BinaryMissing => UsageProbeError::BinaryMissing,
        other => UsageProbeError::Auth(other),
    })?;
    let cwd = probe_cwd()?;
    let env = claude_probe_env(std::env::vars());
    read_usage_from_binary(
        executable.path(),
        &[
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--no-chrome",
            // Skip the Remote Control bridge handshake, observed to add 13s+
            // to a cold start before the idle prompt was usable. `--settings`
            // still applies under `--setting-sources ""`. Do not add
            // CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: it drops the
            // per-model (Fable) week window from the /usage panel.
            "--settings",
            r#"{"remoteControlAtStartup": false}"#,
        ],
        &env,
        &cwd,
        abort,
        PROBE_BUDGET,
    )
}

/// Session workspace. Claude already accepted this folder for the running
/// Rho session; a throwaway cache dir always shows the first-run trust dialog.
fn probe_cwd() -> Result<PathBuf, UsageProbeError> {
    std::env::current_dir().map_err(|error| UsageProbeError::Spawn(error.to_string()))
}

/// Drive `binary` until a completed `/usage` refresh parses. Tests inject a fake child.
pub(crate) fn read_usage_from_binary(
    binary: &Path,
    args: &[&str],
    env: &[(String, String)],
    cwd: &Path,
    abort: &AtomicBool,
    budget: ProbeBudget,
) -> Result<RateLimitState, UsageProbeError> {
    #[cfg(not(unix))]
    {
        let _ = (binary, args, env, cwd, abort, budget);
        return Err(UsageProbeError::Unsupported);
    }
    #[cfg(unix)]
    {
        drive::read_usage_from_binary(binary, args, env, cwd, abort, budget)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdleScreen {
    Trust,
    Login,
    Prompt,
    Other,
}

fn classify_idle_screen(screen: &str) -> IdleScreen {
    let lower = screen.to_ascii_lowercase();
    if contains_any(&lower, TRUST_MARKERS) {
        return IdleScreen::Trust;
    }
    if contains_any(&lower, LOGIN_MARKERS) {
        return IdleScreen::Login;
    }
    if contains_any(&lower, PROMPT_MARKERS) {
        return IdleScreen::Prompt;
    }
    IdleScreen::Other
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// What one `/usage` viewport means. Only `Ready` carries live percentages;
/// every other frame may show placeholder or last-known values.
#[derive(Debug)]
enum UsageScreen {
    /// The panel has not painted yet.
    NoPanel,
    /// Claude reported a failed or degraded refresh.
    Failed,
    /// The spinner is visible; nothing on screen is a live result.
    Refreshing,
    /// The panel names a window that has no percentage yet.
    Incomplete,
    Ready(RateLimitState),
}

fn classify_usage_screen(screen: &str, now_unix: i64) -> UsageScreen {
    let lower = screen.to_ascii_lowercase();
    if contains_any(&lower, REFRESH_FAILURE_MARKERS) {
        return UsageScreen::Failed;
    }
    if !contains_any(screen, PANEL_MARKERS) {
        return UsageScreen::NoPanel;
    }
    if lower.contains("refreshing") {
        return UsageScreen::Refreshing;
    }
    // Never retain an earlier parse: equal-count refreshes replace
    // percentages and may remove windows as well as add them.
    match parse_usage_screen(screen, now_unix) {
        Some(state) if !waiting_on_named_windows(screen, Some(&state)) => UsageScreen::Ready(state),
        _ => UsageScreen::Incomplete,
    }
}

fn waiting_on_named_windows(screen: &str, parsed: Option<&RateLimitState>) -> bool {
    let named = named_window_keys(screen);
    if named.is_empty() {
        return parsed.is_none();
    }
    let have: Vec<&str> = parsed
        .map(|state| {
            state
                .windows
                .iter()
                .map(|window| window.info.window_key())
                .collect()
        })
        .unwrap_or_default();
    named.iter().any(|key| !have.contains(&key.as_str()))
}

fn trust_yes_selected(screen: &str) -> bool {
    screen.lines().any(|line| {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('❯') && !trimmed.starts_with('>') {
            return false;
        }
        let lower = trimmed.to_ascii_lowercase();
        lower.contains("yes") && lower.contains("trust")
    })
}

/// Keep aligned with `rho-tui-pty` `HOST_TERMINAL_MARKERS`. Inherit the rest so
/// keyring / TLS / proxy settings still reach Claude's usage endpoint.
const STRIP_ENV: &[&str] = &[
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

fn claude_probe_env(inherited: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    // Inherit the host environment so keyring, TLS, and proxy settings reach
    // Claude's usage endpoint. A tight allowlist drops those and the panel
    // shows "Failed to load usage data".
    let mut env: Vec<(String, String)> = inherited
        .filter(|(key, _)| {
            !STRIP_ENV
                .iter()
                .any(|strip| strip.eq_ignore_ascii_case(key))
        })
        .collect();
    upsert_env(&mut env, "TERM", "xterm-256color");
    upsert_env(&mut env, "COLORTERM", "truecolor");
    upsert_env(&mut env, "DISABLE_AUTOUPDATER", "1");
    upsert_env(&mut env, "CLAUDE_CODE_AUTO_CONNECT_IDE", "false");
    upsert_env(&mut env, "CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
    // Pin this disposable PTY to classic rendering, independent of the user's
    // fullscreen preference or Claude's automatic renderer fallback. These
    // child-only overrides do not change the user's persistent settings.
    upsert_env(&mut env, "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1");
    upsert_env(&mut env, "CLAUDE_CODE_NO_FLICKER", "0");
    env
}

fn upsert_env(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(existing) = env.iter_mut().find(|(name, _)| name == key) {
        existing.1 = value.into();
        return;
    }
    env.push((key.into(), value.into()));
}

pub(crate) fn live_is_fresh(fetched_at_unix: i64, now_unix: i64) -> bool {
    now_unix.saturating_sub(fetched_at_unix) < i64::try_from(LIVE_TTL.as_secs()).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "usage_probe_tests.rs"]
mod tests;
