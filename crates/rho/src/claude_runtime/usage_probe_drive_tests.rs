use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;

use super::*;

/// Short budgets so hung-child cases fail fast instead of waiting out production values.
const TEST_BUDGET: ProbeBudget = ProbeBudget {
    panel_wait: Duration::from_millis(400),
    grow: Duration::from_millis(400),
};

fn run_child(binary: &str, args: &[&str]) -> Result<RateLimitState, UsageProbeError> {
    let cwd = tempfile::TempDir::new().unwrap();
    run_child_in(binary, args, cwd.path())
}

fn run_child_in(
    binary: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<RateLimitState, UsageProbeError> {
    read_usage_from_binary(
        Path::new(binary),
        args,
        &[("TERM".into(), "xterm-256color".into())],
        cwd,
        &AtomicBool::new(false),
        TEST_BUDGET,
    )
}

/// After `/usage`, paint `first`, wait until that viewport is in the parser
/// and matches `first_kind`, then paint `second` via a stdin handshake. That
/// orders the child paints without a wall-clock delay. `wait_for_usage` may
/// still coalesce them on its first poll; spinner vs ready is covered by
/// `usage_screen_classification`.
fn run_two_paint_child(
    first: &str,
    second: &str,
    first_kind: &str,
) -> Result<RateLimitState, UsageProbeError> {
    let cwd = tempfile::TempDir::new().unwrap();
    let script = format!(
        r#"
printf '? for shortcuts\n'
IFS= read -r command
printf '{first}'
IFS= read -r _
printf '{second}'
exec cat >/dev/null
"#
    );
    let mut session = PtySession::spawn(
        Path::new("/bin/bash"),
        &["-c", &script],
        &[("TERM".into(), "xterm-256color".into())],
        cwd.path(),
        PTY_ROWS,
        PTY_COLS,
    )
    .expect("fake child");
    let abort = AtomicBool::new(false);
    wait_for_prompt(&mut session, &abort)?;
    poll_until(&mut session, &abort, Instant::now() + PROMPT_SETTLE)?;
    session
        .inject_bytes(b"/usage")
        .map_err(UsageProbeError::Spawn)?;
    poll_until(&mut session, &abort, Instant::now() + ENTER_SETTLE)?;
    session
        .inject_bytes(b"\r")
        .map_err(UsageProbeError::Spawn)?;
    let started = Instant::now();
    loop {
        session.poll(POLL_SLICE);
        let screen = session.contents();
        let kind = match classify_usage_screen(&screen, 0) {
            UsageScreen::NoPanel => "NoPanel",
            UsageScreen::Failed => "Failed",
            UsageScreen::Refreshing => "Refreshing",
            UsageScreen::Incomplete => "Incomplete",
            UsageScreen::Ready(_) => "Ready",
        };
        if kind != "NoPanel" {
            assert_eq!(kind, first_kind, "{screen}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "child never painted the first frame: {screen}"
        );
    }
    session
        .inject_bytes(b"\r")
        .map_err(UsageProbeError::Spawn)?;
    wait_for_usage(&mut session, &abort, TEST_BUDGET)
}

fn window_percents(state: &RateLimitState) -> Vec<(&str, Option<f64>)> {
    state
        .sorted_windows()
        .into_iter()
        .map(|window| (window.info.window_key(), window.info.utilization))
        .collect()
}

// Covers: the /usage drive sequence must parse a panel from a fake child.
// Owner: OS or process
#[test]
fn fake_child_usage_panel_parses() {
    let script = r#"
printf '? for shortcuts\n❯ '
buf=
while IFS= read -r -n1 c; do
  buf="${buf}${c}"
  case "$buf" in
    */usage*) break ;;
  esac
done
printf '\nCurrent session\n10%% used\nResets in 1h\nCurrent week (all models)\n20%% used\nResets in 2d\n'
exec cat >/dev/null
"#;
    let state = run_child("/bin/bash", &["-c", script]).expect("fake /usage");
    assert_eq!(
        window_percents(&state),
        vec![("five_hour", Some(0.10)), ("seven_day", Some(0.20))]
    );
}

// Covers: Down+Enter in one write confirms No and Claude exits.
// Owner: OS or process
#[test]
fn fake_child_trust_dialog_needs_split_down_and_enter() {
    let script = r#"
import os, select, sys, tty

def burst(wait):
    fd = sys.stdin.fileno()
    if not select.select([fd], [], [], wait)[0]:
        return b""
    data = os.read(fd, 256)
    while select.select([fd], [], [], 0)[0]:
        chunk = os.read(fd, 256)
        if not chunk:
            break
        data += chunk
    return data

def write(text):
    sys.stdout.write(text)
    sys.stdout.flush()

tty.setraw(sys.stdin.fileno())
write("Accessing workspace:\nYes, I trust this folder\n❯ No, exit\nDo you trust this folder?\n")
if burst(5.0) != b"\x1b[B":
    sys.exit(1)
write("\033[2J\033[HAccessing workspace:\n❯ Yes, I trust this folder\nNo, exit\nDo you trust this folder?\n")
if burst(5.0) != b"\r":
    sys.exit(1)
write("\033[2J\033[H? for shortcuts\n❯ Try \"hi\"\nauto mode on (shift+tab to cycle)\n")
buf = b""
while b"/usage" not in buf:
    chunk = burst(5.0)
    if not chunk:
        sys.exit(1)
    buf += chunk
write("\nCurrent session\n10% used\nResets in 1h\nCurrent week (all models)\n20% used\nResets in 2d\n")
while os.read(sys.stdin.fileno(), 1024):
    pass
"#;
    let state = run_child("/usr/bin/python3", &["-c", script]).expect("fake /usage");
    assert_eq!(
        window_percents(&state),
        vec![("five_hour", Some(0.10)), ("seven_day", Some(0.20))]
    );
}

// Covers: grow must wait for a named window that paints after the first parse.
// The first paint is observed in the parser before the second is released.
// Owner: OS or process
#[test]
fn fake_child_grow_picks_up_late_fable() {
    let state = run_two_paint_child(
        r"\033[2J\033[HCurrent session\n10%% used\nResets in 1h\nCurrent week (all models)\n20%% used\nResets in 2d\nCurrent week (Fable)\n",
        r"\033[2J\033[HCurrent session\n10%% used\nResets in 1h\nCurrent week (all models)\n20%% used\nResets in 2d\nCurrent week (Fable)\n33%% used\nResets in 2d\n",
        "Incomplete",
    )
    .expect("fake /usage");
    assert_eq!(
        window_percents(&state),
        vec![
            ("five_hour", Some(0.10)),
            ("seven_day", Some(0.20)),
            ("seven_day_fable", Some(0.33))
        ]
    );
}

// Covers: a completed refresh replaces every placeholder percentage, even when
// the window count stays the same. The placeholder paint is in the parser
// before the completed panel is released; coalesced later reads still return
// the completed percentages.
// Owner: OS or process
#[test]
fn fake_child_completed_refresh_returns_current_percentages() {
    let state = run_two_paint_child(
        r"\033[2J\033[HCurrent session\n0%% used\nCurrent week (all models)\n0%% used\nCurrent week (Fable)\n0%% used\nRefreshing…\n",
        r"\033[2J\033[HCurrent session\n14%% used\nCurrent week (all models)\n27%% used\nCurrent week (Fable)\n38%% used\nEsc to cancel\n",
        "Refreshing",
    )
    .expect("fake /usage");
    assert_eq!(
        window_percents(&state),
        vec![
            ("five_hour", Some(0.14)),
            ("seven_day", Some(0.27)),
            ("seven_day_fable", Some(0.38))
        ]
    );
}

// Covers: a failure footer under visible percentages is reported, not cached,
// and the diagnostic keeps the footer.
// Owner: OS or process
#[test]
fn fake_child_refresh_failure_rejects_cached_windows() {
    let script = r#"
printf '? for shortcuts\n'
IFS= read -r command
printf '\033[2J\033[HCurrent session\n10%% used\nCurrent week (all models)\n20%% used\nFailed to load usage data: response error\n'
exec cat >/dev/null
"#;
    let result = run_child("/bin/bash", &["-c", script]);
    let Err(UsageProbeError::RefreshFailed { screen }) = result else {
        panic!("{result:?}");
    };
    assert_eq!(
        screen.lines().last(),
        Some("Failed to load usage data: response error")
    );
}

// Covers: a child that exits or hangs on the spinner never promotes
// placeholder percentages.
// Owner: OS or process
#[test]
fn fake_child_unfinished_refresh_never_returns_live() {
    for (ending, expect_exit) in [("exit 0", true), ("exec cat >/dev/null", false)] {
        let script = format!(
            r#"
printf '? for shortcuts\n'
IFS= read -r command
printf '\033[2J\033[HCurrent session\n0%% used\nCurrent week (all models)\n0%% used\nRefreshing…\n'
{ending}
"#
        );
        let (what, screen) = match run_child("/bin/bash", &["-c", &script]) {
            Err(UsageProbeError::Exited { what, screen }) if expect_exit => (what, screen),
            Err(UsageProbeError::TimeoutScreen { what, screen }) if !expect_exit => (what, screen),
            other => panic!("{ending}: {other:?}"),
        };
        assert_eq!(what, "the /usage refresh");
        assert_eq!(screen.lines().last(), Some("Refreshing…"));
    }
}

// Covers: dropping /limits must stop the blocking PTY child.
// Owner: OS or process
#[test]
fn abort_flag_stops_a_hung_child() {
    let abort = std::sync::Arc::new(AtomicBool::new(false));
    let abort_for_thread = abort.clone();
    let started = Instant::now();
    let worker = std::thread::spawn(move || {
        let env = vec![("TERM".into(), "xterm-256color".into())];
        let cwd = tempfile::TempDir::new().unwrap();
        read_usage_from_binary(
            Path::new("/bin/sleep"),
            &["30"],
            &env,
            cwd.path(),
            abort_for_thread.as_ref(),
            TEST_BUDGET,
        )
    });
    std::thread::sleep(Duration::from_millis(80));
    abort.store(true, Ordering::Relaxed);
    let result = worker.join().expect("probe thread");
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert!(
        matches!(result, Err(UsageProbeError::Cancelled)),
        "{result:?}"
    );
}
