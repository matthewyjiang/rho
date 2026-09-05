//! Unix PTY drive for the Claude `/usage` probe.

#[cfg(test)]
#[path = "usage_probe_drive_tests.rs"]
mod tests;

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use super::{
    classify_idle_screen, classify_usage_screen, trust_yes_selected, IdleScreen, ProbeBudget,
    RateLimitState, UsageProbeError, UsageScreen,
};
use crate::claude_runtime::{rate_limit, usage_pty::PtySession};

/// Interactive TUI + keychain + first-run trust dialog + remote-control
/// connect. A warm start reaches the idle prompt in under 1s, but a cold
/// start was captured spending 13s+ on the remote-control handshake alone
/// ("Remote Control disconnected" logged 13s after spawn), so the old 30s
/// budget sat within noise of a spurious "update failed". The probe now
/// disables that handshake via `--settings`, but the budget stays generous
/// as defense-in-depth in case a Claude update stops honoring the flag.
/// Warm runs never touch this; it only bounds a hung child.
const PROMPT_WAIT: Duration = Duration::from_secs(60);
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
/// A PTY read can end before the frame's status footer. A ready panel must
/// hold this long so a trailing "Refreshing" or failure notice is not missed.
const SETTLE_DRAIN: Duration = Duration::from_millis(80);

/// The child is killed on drop; error paths do not need to kill explicitly.
pub(super) fn read_usage_from_binary(
    binary: &Path,
    args: &[&str],
    env: &[(String, String)],
    cwd: &Path,
    abort: &AtomicBool,
    budget: ProbeBudget,
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
    wait_for_usage(&mut session, abort, budget)
}

fn check_abort(abort: &AtomicBool) -> Result<(), UsageProbeError> {
    if abort.load(Ordering::Relaxed) {
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
        check_abort(abort)?;
        session.poll(POLL_SLICE);
    }
    check_abort(abort)
}

fn wait_for_prompt(session: &mut PtySession, abort: &AtomicBool) -> Result<(), UsageProbeError> {
    let mut trust = TrustDrive::NeedDown;
    let deadline = Instant::now() + PROMPT_WAIT;
    loop {
        check_abort(abort)?;
        session.poll(POLL_SLICE);
        let screen = session.contents();
        match classify_idle_screen(&screen) {
            IdleScreen::Prompt => return Ok(()),
            IdleScreen::Login => return Err(UsageProbeError::NotSignedIn),
            IdleScreen::Trust => trust = step_trust(session, trust, &screen)?,
            IdleScreen::Other => {}
        }
        if !session.is_running() {
            return Err(UsageProbeError::Exited {
                what: "the claude prompt",
                screen,
            });
        }
        if Instant::now() >= deadline {
            tracing::debug!(
                screen = %screen,
                "claude usage probe timed out waiting for the idle prompt"
            );
            return Err(UsageProbeError::TimeoutScreen {
                what: "the claude prompt",
                screen,
            });
        }
    }
}

/// Poll until a completed refresh has held for [`SETTLE_DRAIN`], or the child
/// exited on one. Decisions consume the current viewport only, never an
/// earlier parse.
fn wait_for_usage(
    session: &mut PtySession,
    abort: &AtomicBool,
    budget: ProbeBudget,
) -> Result<RateLimitState, UsageProbeError> {
    let deadline = Instant::now() + budget.panel_wait;
    let mut grow_until: Option<Instant> = None;
    let mut ready_since: Option<Instant> = None;
    loop {
        check_abort(abort)?;
        // Liveness before the drain: an exited child's last frame still gets
        // read and classified on this pass.
        let running = session.is_running();
        session.poll(POLL_SLICE);
        let screen = session.contents();
        let now = Instant::now();
        match classify_usage_screen(&screen, rate_limit::now_unix()) {
            UsageScreen::Failed => return Err(UsageProbeError::RefreshFailed { screen }),
            UsageScreen::Ready(state) => {
                grow_until = None;
                if !running || now >= *ready_since.get_or_insert(now) + SETTLE_DRAIN {
                    return Ok(state);
                }
            }
            UsageScreen::Incomplete => {
                ready_since = None;
                if now >= *grow_until.get_or_insert(now + budget.grow) {
                    return Err(UsageProbeError::Unparseable);
                }
            }
            UsageScreen::NoPanel | UsageScreen::Refreshing => {
                grow_until = None;
                ready_since = None;
            }
        }
        if !running {
            return Err(UsageProbeError::Exited {
                what: "the /usage refresh",
                screen,
            });
        }
        if now >= deadline {
            return Err(UsageProbeError::TimeoutScreen {
                what: "the /usage refresh",
                screen,
            });
        }
    }
}

#[derive(Clone, Copy)]
enum TrustDrive {
    NeedDown,
    AwaitYes { down_at: Instant },
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
            Ok(TrustDrive::AwaitYes {
                down_at: Instant::now(),
            })
        }
        TrustDrive::AwaitYes { down_at } => {
            // Default row is "No, exit". Enter only after the pointer is on Yes.
            if trust_yes_selected(screen) {
                session
                    .inject_bytes(TRUST_ENTER)
                    .map_err(UsageProbeError::Spawn)?;
                Ok(TrustDrive::AwaitPrompt {
                    enter_at: Instant::now(),
                })
            } else if Instant::now() >= down_at + TRUST_RETRY {
                Ok(TrustDrive::NeedDown)
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
