use std::{path::Path, sync::atomic::AtomicBool};

use pretty_assertions::assert_eq;

use super::*;

// Covers: cached percentages on failed refreshes must not become live, including
// Claude's seeded fallback that hides Refreshing instead of showing a load error.
// Owner: OS/process. Existing parser tests do not drive refresh status handling.
#[test]
fn fake_child_refresh_failure_rejects_cached_windows() {
    for status in [
        "Failed to load usage data: response error",
        "Showing last-known usage as of 2 minutes ago (could not refresh)",
        "Showing last-known usage (rate limited — try again in a moment)",
        "Partial usage data (rate limited — try again in a moment)",
        "Per-model breakdown unavailable (rate limited — try again in a moment)",
        "Could not refresh usage data",
        "Usage endpoint is rate limited. Please try again in a moment.",
    ] {
        let script = format!(
            r#"
printf '? for shortcuts\n'
IFS= read -r command
printf '\033[2J\033[HCurrent session\n10%% used\nCurrent week (all models)\n20%% used\nCurrent week (Fable)\n30%% used\n{status}\n'
exec cat >/dev/null
"#
        );
        let result = run_child(&script);
        let Err(UsageProbeError::RefreshFailed { screen }) = result else {
            panic!("{status}: {result:?}");
        };
        // The diagnostic must retain the footer, not truncate it after windows.
        assert_eq!(screen.lines().last(), Some(status));
    }
}

fn run_child(script: &str) -> Result<RateLimitState, UsageProbeError> {
    let cwd = tempfile::TempDir::new().unwrap();
    read_usage_from_binary(
        Path::new("/bin/bash"),
        &["-c", script],
        &[("TERM".into(), "xterm-256color".into())],
        cwd.path(),
        &AtomicBool::new(false),
        super::super::PANEL_GROW,
    )
}

// Covers: ending or exhausting the refresh budget cannot promote placeholder
// percentages. One hung child exercises the real deadline without sleep-sync.
// Owner: OS/process.
#[test]
fn fake_child_unfinished_refresh_never_returns_live() {
    for ending in ["exit 0", "exec cat >/dev/null"] {
        let script = format!(
            r#"
printf '? for shortcuts\n'
IFS= read -r command
printf '\033[2J\033[HCurrent session\n0%% used\nCurrent week (all models)\n0%% used\nCurrent week (Fable)\n0%% used\nRefreshing…\n'
{ending}
"#
        );
        let result = run_child(&script);
        let Err(UsageProbeError::TimeoutScreen { what, screen }) = result else {
            panic!("{ending}: {result:?}");
        };
        assert_eq!(what, "the /usage refresh");
        assert_eq!(screen.lines().last(), Some("Refreshing…"));
    }
}

// Covers: the completed terminal snapshot replaces all placeholder values,
// including when the number of windows stays the same. Child input is the
// synchronization signal between paints, not a wall-clock delay.
// Owner: OS/process.
#[test]
fn fake_child_completed_refresh_returns_current_percentages() {
    let script = r#"
import os, tty
tty.setraw(0)
def paint(text):
    os.write(1, text.encode())
paint('\033[2J\033[HCurrent session\r\n0% used\r\nCurrent week (all models)\r\n0% used\r\nCurrent week (Fable)\r\n0% used\r\nRefreshing…\r\n')
while os.read(0, 1) != b'\r':
    pass
paint('\033[2J\033[HCurrent session\r\n14% used\r\nCurrent week (all models)\r\n27% used\r\nCurrent week (Fable)\r\n38% used\r\nEsc to cancel\r\n')
while os.read(0, 1):
    pass
"#;
    let cwd = tempfile::TempDir::new().unwrap();
    let mut session = PtySession::spawn(
        Path::new("/usr/bin/python3"),
        &["-c", script],
        &[("TERM".into(), "xterm-256color".into())],
        cwd.path(),
        PTY_ROWS,
        PTY_COLS,
    )
    .expect("fake child");
    let deadline = Instant::now() + PANEL_WAIT;
    let mut collection = UsageCollection::new(super::super::PANEL_GROW, deadline);
    wait_for_footer(&mut session, "Refreshing…", deadline);
    assert!(matches!(
        collection.observe(&session.contents(), session.is_running(), Instant::now()),
        Ok(CollectStep::Poll)
    ));

    // The child cannot paint fresh values until the collector has actually
    // observed and rejected the complete three-window placeholder frame.
    session.inject_bytes(b"\r").unwrap();
    wait_for_footer(&mut session, "Esc to cancel", deadline);
    assert!(matches!(
        collection.observe(&session.contents(), session.is_running(), Instant::now()),
        Ok(CollectStep::Drain)
    ));
    poll_until(
        &mut session,
        &AtomicBool::new(false),
        Instant::now() + SETTLE_DRAIN,
    )
    .unwrap();
    let Ok(CollectStep::Ready(state)) =
        collection.observe(&session.contents(), session.is_running(), Instant::now())
    else {
        panic!("completed refresh was not ready");
    };
    assert_eq!(
        state
            .sorted_windows()
            .into_iter()
            .map(|window| (window.info.window_key(), window.info.utilization))
            .collect::<Vec<_>>(),
        vec![
            ("five_hour", Some(0.14)),
            ("seven_day", Some(0.27)),
            ("seven_day_fable", Some(0.38))
        ]
    );
}

fn wait_for_footer(session: &mut PtySession, footer: &str, deadline: Instant) {
    while session.contents().lines().last() != Some(footer) {
        assert!(Instant::now() < deadline, "child never painted {footer}");
        session.poll(POLL_SLICE);
    }
}
