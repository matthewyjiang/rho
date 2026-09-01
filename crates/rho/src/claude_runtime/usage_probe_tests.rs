use super::{classify_idle_screen, trust_yes_selected, waiting_on_named_windows, IdleScreen};

// Covers: the trust dialog's ❯ must not be treated as the idle prompt.
// Owner: pure unit
#[test]
fn trust_dialog_is_not_a_ready_prompt() {
    let screen = r#"
Accessing workspace:
Quick safety check
❯ No, exit
Yes, I trust this folder
Enter to confirm
"#;
    assert_eq!(classify_idle_screen(screen), IdleScreen::Trust);
}

// Covers: Enter on the default No row must not be treated as Yes.
// Owner: pure unit
#[test]
fn trust_yes_requires_the_pointer_on_yes() {
    let no = r#"
Accessing workspace:
Yes, I trust this folder
❯ No, exit
Do you trust this folder?
"#;
    assert!(!trust_yes_selected(no));
    let yes = r#"
Accessing workspace:
❯ Yes, I trust this folder
No, exit
Do you trust this folder?
"#;
    assert!(trust_yes_selected(yes));
}

// Covers: session+week paint must not finish while Fable is named without %.
// Owner: pure unit
#[test]
fn waits_while_named_fable_header_has_no_percent() {
    let partial =
        super::super::usage_parse::parse_usage_screen("Current session\n0%used\nResets in 1h\n", 0);
    assert!(waiting_on_named_windows(
        "Current session\n0%used\nCurrent week (Fable)\n",
        partial.as_ref()
    ));
    let complete = super::super::usage_parse::parse_usage_screen(
        "Current session\n0%used\nResets in 1h\nCurrent week (Fable)\n33%used\nResets in 2d\n",
        0,
    );
    assert!(!waiting_on_named_windows(
        "Current session\n0%used\nCurrent week (Fable)\n33%used\n",
        complete.as_ref()
    ));
}

#[cfg(unix)]
mod pty {
    use std::{path::Path, sync::atomic::AtomicBool};

    use pretty_assertions::assert_eq;

    use super::super::read_usage_from_binary;

    fn run_cmd(binary: &str, args: &[&str]) -> super::super::RateLimitState {
        run_cmd_grow(binary, args, std::time::Duration::from_millis(400))
    }

    fn run_cmd_grow(
        binary: &str,
        args: &[&str],
        grow: std::time::Duration,
    ) -> super::super::RateLimitState {
        let env = vec![("TERM".into(), "xterm-256color".into())];
        let cwd = tempfile::TempDir::new().unwrap();
        read_usage_from_binary(
            Path::new(binary),
            args,
            &env,
            cwd.path(),
            &AtomicBool::new(false),
            grow,
        )
        .expect("fake /usage")
    }

    fn assert_session_and_week(state: &super::super::RateLimitState) {
        let keys: Vec<&str> = state
            .sorted_windows()
            .into_iter()
            .map(|window| window.info.window_key())
            .collect();
        assert_eq!(keys, vec!["five_hour", "seven_day"]);
        assert_eq!(state.sorted_windows()[0].info.utilization, Some(0.10));
        assert_eq!(state.sorted_windows()[1].info.utilization, Some(0.20));
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
        assert_session_and_week(&run_cmd("/bin/bash", &["-c", script]));
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
        assert_session_and_week(&run_cmd("/usr/bin/python3", &["-c", script]));
    }

    // Covers: grow must wait for a named window that paints after the first parse.
    // Owner: OS or process
    #[test]
    fn fake_child_grow_picks_up_late_fable() {
        let script = r#"
printf '? for shortcuts\n❯ '
buf=
while IFS= read -r -n1 c; do
  buf="${buf}${c}"
  case "$buf" in
    */usage*) break ;;
  esac
done
printf '\nCurrent session\n10%% used\nResets in 1h\nCurrent week (all models)\n20%% used\nResets in 2d\nCurrent week (Fable)\n'
sleep 0.2
printf '\nCurrent session\n10%% used\nResets in 1h\nCurrent week (all models)\n20%% used\nResets in 2d\nCurrent week (Fable)\n33%% used\nResets in 2d\n'
exec cat >/dev/null
"#;
        let state = run_cmd_grow(
            "/bin/bash",
            &["-c", script],
            std::time::Duration::from_millis(800),
        );
        let keys: Vec<&str> = state
            .sorted_windows()
            .into_iter()
            .map(|window| window.info.window_key())
            .collect();
        assert_eq!(keys, vec!["five_hour", "seven_day", "seven_day_fable"]);
        assert_eq!(state.sorted_windows()[2].info.utilization, Some(0.33));
    }

    // Covers: dropping /limits must stop the blocking PTY child.
    // Owner: OS or process
    #[test]
    fn abort_flag_stops_a_hung_child() {
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let abort = std::sync::Arc::new(AtomicBool::new(false));
        let abort_for_thread = abort.clone();
        let started = Instant::now();
        let worker = std::thread::spawn(move || {
            let env = vec![("TERM".into(), "xterm-256color".into())];
            let cwd = tempfile::TempDir::new().unwrap();
            super::super::read_usage_from_binary(
                Path::new("/bin/sleep"),
                &["30"],
                &env,
                cwd.path(),
                abort_for_thread.as_ref(),
                Duration::from_secs(2),
            )
        });
        std::thread::sleep(Duration::from_millis(80));
        abort.store(true, Ordering::Relaxed);
        let result = worker.join().expect("probe thread");
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        assert!(
            matches!(result, Err(super::super::UsageProbeError::Cancelled)),
            "{result:?}"
        );
    }
}
