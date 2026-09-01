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

#[cfg(unix)]
use std::time::Instant;

use thiserror::Error;

use super::{
    auth::{self, ClaudeAuthError},
    executable,
    rate_limit::{self, RateLimitState},
};

#[cfg(any(unix, test))]
use super::usage_parse::named_window_keys;
#[cfg(unix)]
use super::usage_parse::parse_usage_screen;

/// Interactive TUI + keychain + first-run trust dialog. JSON `auth status`
/// is 10s without a TUI; local capture needed ~13s to reach the idle prompt.
#[cfg(unix)]
const PROMPT_WAIT: Duration = Duration::from_secs(30);
/// `/usage` then Anthropic's usage endpoint.
#[cfg(unix)]
const PANEL_WAIT: Duration = Duration::from_secs(15);
/// A full TUI start is ~10–20s. Reuse a successful probe for the rest of a
/// work burst so `/limits` does not pay that again. Claude's own last-known
/// `/usage` window is 60 minutes; five minutes stays live without stacking
/// Claude processes.
pub(crate) const LIVE_TTL: Duration = Duration::from_secs(5 * 60);

#[cfg(any(unix, test))]
const PROMPT_MARKERS: &[&str] = &["? for shortcuts", "try \"", "shift+tab to cycle"];
#[cfg(any(unix, test))]
const TRUST_MARKERS: &[&str] = &["trust this folder", "do you trust"];
#[cfg(unix)]
const PANEL_MARKERS: &[&str] = &["Current session", "% used", "%used"];
/// Wait this long only while the screen names a window we have not parsed.
const PANEL_GROW: Duration = Duration::from_secs(2);
#[cfg(any(unix, test))]
const LOGIN_MARKERS: &[&str] = &["log in", "sign in to"];
/// Trust dialog defaults to "No, exit". Down and Enter must be separate
/// writes; one burst of Down+Enter confirms No and Claude exits.
#[cfg(unix)]
const TRUST_DOWN: &[u8] = b"\x1b[B";
#[cfg(unix)]
const TRUST_ENTER: &[u8] = b"\r";
#[cfg(unix)]
const TRUST_ARROW_SETTLE: Duration = Duration::from_millis(150);
#[cfg(unix)]
const TRUST_RETRY: Duration = Duration::from_millis(400);

#[cfg(unix)]
const PTY_ROWS: u16 = 36;
#[cfg(unix)]
const PTY_COLS: u16 = 140;
#[cfg(unix)]
const PROMPT_SETTLE: Duration = Duration::from_millis(50);
#[cfg(unix)]
const ENTER_SETTLE: Duration = Duration::from_millis(80);

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
    #[error("claude code: timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("claude code: timed out waiting for {what}: {screen}")]
    TimeoutScreen { what: &'static str, screen: String },
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
///
/// Tests never launch the real `claude` TUI; fake-child coverage goes through
/// [`read_usage_from_binary`].
pub(crate) fn probe_supported() -> bool {
    cfg!(all(unix, not(test)))
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
    for window in &mut state.windows {
        window.observed_at_unix = now;
    }
    if let Ok(path) = rate_limit::default_state_path() {
        if let Ok(merged) = rate_limit::store_state(&path, state.clone()) {
            return Ok(UsageProbeOutcome::Ready(merged));
        }
    }
    Ok(UsageProbeOutcome::Ready(state))
}

fn probe_usage_blocking(abort: &AtomicBool) -> Result<RateLimitState, UsageProbeError> {
    let executable = executable::resolve().map_err(|error| match error {
        ClaudeAuthError::BinaryMissing => UsageProbeError::BinaryMissing,
        other => UsageProbeError::Auth(other),
    })?;
    let cwd = probe_cwd()?;
    let env = claude_probe_env();
    read_usage_from_binary(
        executable.path(),
        &[
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--no-chrome",
        ],
        &env,
        &cwd,
        abort,
        PANEL_GROW,
    )
}

/// Session workspace. Claude already accepted this folder for the running
/// Rho session; a throwaway cache dir always shows the first-run trust dialog.
fn probe_cwd() -> Result<PathBuf, UsageProbeError> {
    std::env::current_dir().map_err(|error| UsageProbeError::Spawn(error.to_string()))
}

/// Drive `binary` until a `/usage` panel parses. Tests inject a fake child.
pub(crate) fn read_usage_from_binary(
    binary: &Path,
    args: &[&str],
    env: &[(String, String)],
    cwd: &Path,
    abort: &AtomicBool,
    grow: Duration,
) -> Result<RateLimitState, UsageProbeError> {
    #[cfg(not(unix))]
    {
        let _ = (binary, args, env, cwd, abort, grow);
        return Err(UsageProbeError::Unsupported);
    }
    #[cfg(unix)]
    {
        read_usage_from_binary_unix(binary, args, env, cwd, abort, grow)
    }
}

#[cfg(unix)]
fn read_usage_from_binary_unix(
    binary: &Path,
    args: &[&str],
    env: &[(String, String)],
    cwd: &Path,
    abort: &AtomicBool,
    grow: Duration,
) -> Result<RateLimitState, UsageProbeError> {
    let mut session =
        super::usage_pty::PtySession::spawn(binary, args, env, cwd, PTY_ROWS, PTY_COLS)
            .map_err(UsageProbeError::Spawn)?;
    wait_for_prompt(&mut session, abort)?;
    poll_until(&mut session, abort, Instant::now() + PROMPT_SETTLE)?;
    session
        .inject_bytes(b"/usage")
        .map_err(UsageProbeError::Spawn)?;
    poll_until(&mut session, abort, Instant::now() + ENTER_SETTLE)?;
    session
        .inject_bytes(b"\r")
        .map_err(UsageProbeError::Spawn)?;
    wait_for_usage_panel(&mut session, abort)?;
    collect_usage(&mut session, abort, grow)
}

#[cfg(unix)]
fn kill_if_aborted(
    session: &mut super::usage_pty::PtySession,
    abort: &AtomicBool,
) -> Result<(), UsageProbeError> {
    if abort.load(Ordering::Relaxed) {
        session.kill();
        return Err(UsageProbeError::Cancelled);
    }
    Ok(())
}

#[cfg(unix)]
fn poll_until(
    session: &mut super::usage_pty::PtySession,
    abort: &AtomicBool,
    until: Instant,
) -> Result<(), UsageProbeError> {
    while Instant::now() < until {
        kill_if_aborted(session, abort)?;
        session.poll(Duration::from_millis(25));
    }
    kill_if_aborted(session, abort)
}

#[cfg(unix)]
fn wait_for_prompt(
    session: &mut super::usage_pty::PtySession,
    abort: &AtomicBool,
) -> Result<(), UsageProbeError> {
    let mut trust = TrustDrive::NeedDown;
    let deadline = Instant::now() + PROMPT_WAIT;
    loop {
        kill_if_aborted(session, abort)?;
        session.poll(Duration::from_millis(25));
        let screen = session.contents();
        match classify_idle_screen(&screen) {
            IdleScreen::Prompt => return Ok(()),
            IdleScreen::Login => {
                session.kill();
                return Err(UsageProbeError::NotSignedIn);
            }
            kind => {
                if kind == IdleScreen::Trust {
                    trust = step_trust(session, trust, &screen)?;
                }
                if !session.is_running() {
                    session.kill();
                    return Err(UsageProbeError::Timeout("the claude prompt"));
                }
                if Instant::now() >= deadline {
                    tracing::debug!(
                        screen = %session.contents(),
                        "claude usage probe timed out waiting for the idle prompt"
                    );
                    session.kill();
                    return Err(UsageProbeError::TimeoutScreen {
                        what: "the claude prompt",
                        screen: session.contents().chars().take(800).collect(),
                    });
                }
            }
        }
    }
}

#[cfg(unix)]
fn wait_for_usage_panel(
    session: &mut super::usage_pty::PtySession,
    abort: &AtomicBool,
) -> Result<(), UsageProbeError> {
    let deadline = Instant::now() + PANEL_WAIT;
    let mut retried = false;
    loop {
        kill_if_aborted(session, abort)?;
        session.poll(Duration::from_millis(25));
        let screen = session.contents();
        if PANEL_MARKERS.iter().any(|needle| screen.contains(needle)) {
            return Ok(());
        }
        if !retried && screen.to_ascii_lowercase().contains("failed to load usage") {
            session.inject_bytes(b"r").map_err(UsageProbeError::Spawn)?;
            retried = true;
            continue;
        }
        if !session.is_running() {
            session.poll(Duration::from_millis(50));
            let screen = session.contents();
            if PANEL_MARKERS.iter().any(|needle| screen.contains(needle)) {
                return Ok(());
            }
            session.kill();
            return Err(UsageProbeError::TimeoutScreen {
                what: "the /usage panel",
                screen: screen.chars().take(800).collect(),
            });
        }
        if Instant::now() >= deadline {
            session.kill();
            return Err(UsageProbeError::TimeoutScreen {
                what: "the /usage panel",
                screen: session.contents().chars().take(800).collect(),
            });
        }
    }
}

#[cfg(unix)]
enum CollectTick {
    Done(RateLimitState),
    Continue {
        best: Option<RateLimitState>,
        screen: String,
        running: bool,
    },
}

#[cfg(unix)]
fn poll_and_parse(
    session: &mut super::usage_pty::PtySession,
    abort: &AtomicBool,
    mut best: Option<RateLimitState>,
) -> Result<CollectTick, UsageProbeError> {
    kill_if_aborted(session, abort)?;
    session.poll(Duration::from_millis(25));
    let screen = session.contents();
    let running = session.is_running();
    if let Some(state) = parse_usage_screen(&screen, rate_limit::now_unix()) {
        if best.as_ref().map_or(0, |ready| ready.windows.len()) < state.windows.len() {
            best = Some(state);
        }
    }
    if !waiting_on_named_windows(&screen, best.as_ref()) {
        if let Some(state) = best.take() {
            session.kill();
            return Ok(CollectTick::Done(state));
        }
    }
    Ok(CollectTick::Continue {
        best,
        screen,
        running,
    })
}

#[cfg(unix)]
fn collect_usage(
    session: &mut super::usage_pty::PtySession,
    abort: &AtomicBool,
    grow: Duration,
) -> Result<RateLimitState, UsageProbeError> {
    let grow_until = Instant::now() + grow;
    let mut best: Option<RateLimitState> = None;
    loop {
        match poll_and_parse(session, abort, best)? {
            CollectTick::Done(state) => return Ok(state),
            CollectTick::Continue {
                best: next,
                running,
                ..
            } => {
                best = next;
                if Instant::now() >= grow_until || !running {
                    if let Some(state) = best.take() {
                        session.kill();
                        return Ok(state);
                    }
                    return wait_until_refresh_settles(session, abort);
                }
            }
        }
    }
}

#[cfg(unix)]
fn wait_until_refresh_settles(
    session: &mut super::usage_pty::PtySession,
    abort: &AtomicBool,
) -> Result<RateLimitState, UsageProbeError> {
    let until = Instant::now() + Duration::from_secs(8);
    let mut best: Option<RateLimitState> = None;
    loop {
        match poll_and_parse(session, abort, best)? {
            CollectTick::Done(state) => return Ok(state),
            CollectTick::Continue {
                best: next,
                screen,
                running,
            } => {
                best = next;
                let refreshing = screen.to_ascii_lowercase().contains("refreshing");
                if !refreshing || !running || Instant::now() >= until {
                    session.poll(Duration::from_millis(80));
                    let screen = session.contents();
                    session.kill();
                    return parse_usage_screen(&screen, rate_limit::now_unix())
                        .ok_or(UsageProbeError::Unparseable);
                }
            }
        }
    }
}

#[cfg(any(unix, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdleScreen {
    Trust,
    Login,
    Prompt,
    Other,
}

#[cfg(any(unix, test))]
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

#[cfg(any(unix, test))]
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(any(unix, test))]
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

#[cfg(any(unix, test))]
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

#[cfg(unix)]
#[derive(Clone, Copy)]
enum TrustDrive {
    NeedDown,
    AwaitYes { down_at: Instant },
    AwaitPrompt { enter_at: Instant },
}

#[cfg(unix)]
fn step_trust(
    session: &mut super::usage_pty::PtySession,
    drive: TrustDrive,
    screen: &str,
) -> Result<TrustDrive, UsageProbeError> {
    match drive {
        TrustDrive::NeedDown => {
            session
                .inject_bytes(TRUST_DOWN)
                .map_err(UsageProbeError::Spawn)?;
            Ok(TrustDrive::AwaitYes {
                down_at: Instant::now(),
            })
        }
        TrustDrive::AwaitYes { down_at } => {
            if trust_yes_selected(screen) || Instant::now() >= down_at + TRUST_ARROW_SETTLE {
                session
                    .inject_bytes(TRUST_ENTER)
                    .map_err(UsageProbeError::Spawn)?;
                Ok(TrustDrive::AwaitPrompt {
                    enter_at: Instant::now(),
                })
            } else {
                Ok(drive)
            }
        }
        TrustDrive::AwaitPrompt { enter_at } => {
            if Instant::now() >= enter_at + TRUST_RETRY {
                Ok(TrustDrive::NeedDown)
            } else {
                Ok(drive)
            }
        }
    }
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

fn claude_probe_env() -> Vec<(String, String)> {
    // Inherit the host environment so keyring, TLS, and proxy settings reach
    // Claude's usage endpoint. A tight allowlist drops those and the panel
    // shows "Failed to load usage data".
    let mut env: Vec<(String, String)> = std::env::vars()
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
