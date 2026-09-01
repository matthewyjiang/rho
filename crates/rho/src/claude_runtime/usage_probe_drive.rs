//! Unix PTY drive for the Claude `/usage` probe.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use super::{
    classify_idle_screen, trust_yes_selected, waiting_on_named_windows, IdleScreen, RateLimitState,
    UsageProbeError,
};
use crate::claude_runtime::{rate_limit, usage_parse::parse_usage_screen, usage_pty::PtySession};

/// Interactive TUI + keychain + first-run trust dialog. JSON `auth status`
/// is 10s without a TUI; local capture needed ~13s to reach the idle prompt.
const PROMPT_WAIT: Duration = Duration::from_secs(30);
/// `/usage` then Anthropic's usage endpoint.
const PANEL_WAIT: Duration = Duration::from_secs(15);
/// Anthropic's usage endpoint can sit on "Refreshing" after the panel
/// appears. Eight seconds covers a slow network without rivaling
/// [`PANEL_WAIT`] on a hung refresh.
const REFRESH_SETTLE: Duration = Duration::from_secs(8);
const PANEL_MARKERS: &[&str] = &["Current session", "% used", "%used"];
/// Trust dialog defaults to "No, exit". Down and Enter must be separate
/// writes; one burst of Down+Enter confirms No and Claude exits.
const TRUST_DOWN: &[u8] = b"\x1b[B";
const TRUST_ENTER: &[u8] = b"\r";
const TRUST_RETRY: Duration = Duration::from_millis(400);
const PTY_ROWS: u16 = 36;
const PTY_COLS: u16 = 140;
const PROMPT_SETTLE: Duration = Duration::from_millis(50);
const ENTER_SETTLE: Duration = Duration::from_millis(80);
const POLL_SLICE: Duration = Duration::from_millis(25);
const SETTLE_DRAIN: Duration = Duration::from_millis(80);

enum CollectPhase {
    Grow,
    Settle,
}

pub(super) fn read_usage_from_binary(
    binary: &Path,
    args: &[&str],
    env: &[(String, String)],
    cwd: &Path,
    abort: &AtomicBool,
    grow: Duration,
) -> Result<RateLimitState, UsageProbeError> {
    let mut session = PtySession::spawn(binary, args, env, cwd, PTY_ROWS, PTY_COLS)
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

fn kill_if_aborted(session: &mut PtySession, abort: &AtomicBool) -> Result<(), UsageProbeError> {
    if abort.load(Ordering::Relaxed) {
        session.kill();
        return Err(UsageProbeError::Cancelled);
    }
    Ok(())
}

fn poll_until(
    session: &mut PtySession,
    abort: &AtomicBool,
    until: Instant,
) -> Result<(), UsageProbeError> {
    while Instant::now() < until {
        kill_if_aborted(session, abort)?;
        session.poll(POLL_SLICE);
    }
    kill_if_aborted(session, abort)
}

fn wait_for_prompt(session: &mut PtySession, abort: &AtomicBool) -> Result<(), UsageProbeError> {
    let mut trust = TrustDrive::NeedDown;
    let deadline = Instant::now() + PROMPT_WAIT;
    loop {
        kill_if_aborted(session, abort)?;
        session.poll(POLL_SLICE);
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

fn wait_for_usage_panel(
    session: &mut PtySession,
    abort: &AtomicBool,
) -> Result<(), UsageProbeError> {
    let deadline = Instant::now() + PANEL_WAIT;
    let mut retried = false;
    loop {
        kill_if_aborted(session, abort)?;
        session.poll(POLL_SLICE);
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

fn consider_parse(best: &mut Option<RateLimitState>, screen: &str) {
    if let Some(state) = parse_usage_screen(screen, rate_limit::now_unix()) {
        if best.as_ref().map_or(0, |ready| ready.windows.len()) < state.windows.len() {
            *best = Some(state);
        }
    }
}

fn finish_with_best(
    session: &mut PtySession,
    best: Option<RateLimitState>,
    screen: &str,
) -> Result<RateLimitState, UsageProbeError> {
    session.kill();
    best.or_else(|| parse_usage_screen(screen, rate_limit::now_unix()))
        .ok_or(UsageProbeError::Unparseable)
}

fn collect_usage(
    session: &mut PtySession,
    abort: &AtomicBool,
    grow: Duration,
) -> Result<RateLimitState, UsageProbeError> {
    let mut phase = CollectPhase::Grow;
    let mut deadline = Instant::now() + grow;
    let mut best: Option<RateLimitState> = None;
    loop {
        kill_if_aborted(session, abort)?;
        session.poll(POLL_SLICE);
        let screen = session.contents();
        let running = session.is_running();
        consider_parse(&mut best, &screen);
        if !waiting_on_named_windows(&screen, best.as_ref()) {
            if let Some(state) = best.take() {
                session.kill();
                return Ok(state);
            }
        }
        let timed_out = Instant::now() >= deadline;
        match phase {
            CollectPhase::Grow if timed_out || !running => {
                if let Some(state) = best.take() {
                    session.kill();
                    return Ok(state);
                }
                phase = CollectPhase::Settle;
                deadline = Instant::now() + REFRESH_SETTLE;
            }
            CollectPhase::Settle => {
                let refreshing = screen.to_ascii_lowercase().contains("refreshing");
                if !refreshing || !running || timed_out {
                    session.poll(SETTLE_DRAIN);
                    let screen = session.contents();
                    consider_parse(&mut best, &screen);
                    return finish_with_best(session, best, &screen);
                }
            }
            CollectPhase::Grow => {}
        }
    }
}

#[derive(Clone, Copy)]
enum TrustDrive {
    NeedDown,
    AwaitYes,
    AwaitPrompt { enter_at: Instant },
}

fn step_trust(
    session: &mut PtySession,
    drive: TrustDrive,
    screen: &str,
) -> Result<TrustDrive, UsageProbeError> {
    match drive {
        TrustDrive::NeedDown => {
            session
                .inject_bytes(TRUST_DOWN)
                .map_err(UsageProbeError::Spawn)?;
            Ok(TrustDrive::AwaitYes)
        }
        TrustDrive::AwaitYes => {
            // Default row is "No, exit". Enter only after the pointer is on Yes.
            if trust_yes_selected(screen) {
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
