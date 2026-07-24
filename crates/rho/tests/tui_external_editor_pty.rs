//! External editor behavior driven through a real pseudo-terminal.

#![cfg(unix)]

use std::{path::PathBuf, time::Duration};

use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};

const ALTERNATE_SCREEN_ENTER: &[u8] = b"\x1b[?1049h";

#[test]
fn composer_round_trips_through_interactive_external_editor() {
    use std::os::unix::fs::PermissionsExt;

    let home = IsolatedHome::new().unwrap();
    let editor_dir = tempfile::Builder::new()
        .prefix("rho editor test ")
        .tempdir()
        .unwrap();
    let editor = editor_dir.path().join("interactive editor");
    let terminal_state = editor_dir.path().join("terminal state");
    let original_draft = editor_dir.path().join("original draft");
    std::fs::write(
        &editor,
        r#"#!/bin/sh
stty -a > "$1"
cat "$3" > "$2"
printf 'EXTERNAL_EDITOR_READY\n'
IFS= read -r replacement
printf '%s\n' "$replacement" > "$3"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let editor_command = format!(
        "'{}' '{}' '{}'",
        editor.display(),
        terminal_state.display(),
        original_draft.display()
    );
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("EDITOR", editor_command);
    let mut harness = PtyHarness::spawn_named(&plan, "external_editor").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.paste("alpha\nbeta").unwrap();
    harness.settle_input();
    let expected_screen_entries = harness.raw_sequence_occurrences(ALTERNATE_SCREEN_ENTER) + 1;
    harness.inject_key(&Key::Ctrl('g')).unwrap();
    harness
        .wait_for_text(
            "EXTERNAL_EDITOR_READY",
            WaitTimeout::secs(10, "interactive editor ready"),
        )
        .unwrap();
    harness.type_text("edited in external editor").unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_raw_sequence_occurrences(
            ALTERNATE_SCREEN_ENTER,
            expected_screen_entries,
            WaitTimeout::secs(10, "TUI resumed after editor"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "edited in external editor",
            WaitTimeout::secs(10, "editor return"),
        )
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(&original_draft).unwrap(),
        "alpha\nbeta"
    );
    let state = std::fs::read_to_string(&terminal_state).unwrap();
    assert!(
        state
            .split_ascii_whitespace()
            .map(|word| word.trim_matches(';'))
            .any(|word| word == "icanon"),
        "editor inherited non-canonical terminal state: {state}"
    );
    let raw = harness.raw_output();
    let leave_at = raw
        .windows(8)
        .position(|bytes| bytes == b"\x1b[?1049l")
        .expect("leave alternate screen");
    let clear_at = raw[leave_at..]
        .windows(4)
        .position(|bytes| bytes == b"\x1b[2J")
        .map(|offset| leave_at + offset)
        .expect("main screen clear after leaving alternate screen");
    let opening_at = raw
        .windows(b"Opening editor".len())
        .position(|bytes| bytes == b"Opening editor")
        .expect("opening editor status");
    let ready_at = raw
        .windows(b"EXTERNAL_EDITOR_READY".len())
        .position(|bytes| bytes == b"EXTERNAL_EDITOR_READY")
        .expect("external editor ready marker");
    assert!(
        leave_at < clear_at && clear_at < opening_at && opening_at < ready_at,
        "handoff should leave alternate screen, clear main buffer, show status, then start the editor"
    );
    assert!(
        raw.windows(8)
            .filter(|bytes| *bytes == b"\x1b[?1049h")
            .count()
            >= 2,
        "alternate screen was not re-entered after the editor"
    );

    harness.settle_input();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "fixture response: edited in external editor",
            WaitTimeout::secs(20, "edited prompt response"),
        )
        .unwrap();

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn external_editor_errors_preserve_the_composer() {
    use std::os::unix::fs::PermissionsExt;

    let editor_dir = tempfile::tempdir().unwrap();
    let failing_editor = editor_dir.path().join("failing-editor");
    std::fs::write(&failing_editor, "#!/bin/sh\nexit 7\n").unwrap();
    std::fs::set_permissions(&failing_editor, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cases = [
        ("unset", None, "EDITOR is not set"),
        ("empty", Some(String::new()), "EDITOR is empty"),
        (
            "missing",
            Some("/rho/does/not/exist".into()),
            "could not start EDITOR",
        ),
        (
            "nonzero",
            Some(failing_editor.display().to_string()),
            "EDITOR exited with exit status: 7",
        ),
    ];

    for (name, editor, expected_error) in cases {
        let home = IsolatedHome::new().unwrap();
        let mut plan = RhoLaunchPlan::matrix(
            PathBuf::from(env!("CARGO_BIN_EXE_rho")),
            &home,
            PtySize {
                rows: 28,
                cols: 100,
            },
        );
        if let Some(editor) = editor {
            plan = plan.with_env("EDITOR", editor);
        }
        let mut harness =
            PtyHarness::spawn_named(&plan, format!("external_editor_{name}")).unwrap();
        harness
            .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
            .unwrap();
        let draft = format!("preserved {name} draft");
        harness.type_text(&draft).unwrap();
        harness.inject_key(&Key::Ctrl('g')).unwrap();
        harness
            .wait_for_text(expected_error, WaitTimeout::secs(10, "editor error status"))
            .unwrap();
        harness.settle_input();
        harness.inject_key(&Key::Enter).unwrap();
        harness
            .wait_for_text(
                &format!("fixture response: {draft}"),
                WaitTimeout::secs(20, "preserved draft response"),
            )
            .unwrap();
        assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
    }
}

#[test]
fn editor_job_control_signals_do_not_interrupt_rho() {
    use std::os::unix::fs::PermissionsExt;

    let home = IsolatedHome::new().unwrap();
    let editor_dir = tempfile::tempdir().unwrap();
    let editor = editor_dir.path().join("interruptible-editor");
    std::fs::write(
        &editor,
        r#"#!/bin/sh
trap 'exit 130' INT
trap 'printf "INTERRUPTIBLE_EDITOR_CONTINUED\\n"' CONT
printf 'INTERRUPTIBLE_EDITOR_READY\n'
while :; do sleep 1; done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
    let plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("EDITOR", editor.display().to_string());
    let mut harness = PtyHarness::spawn_named(&plan, "external_editor_ctrl_c").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness
        .type_text("draft survives editor interrupt")
        .unwrap();
    let expected_screen_entries = harness.raw_sequence_occurrences(ALTERNATE_SCREEN_ENTER) + 1;
    harness.inject_key(&Key::Ctrl('g')).unwrap();
    harness
        .wait_for_text(
            "INTERRUPTIBLE_EDITOR_READY",
            WaitTimeout::secs(10, "editor ready"),
        )
        .unwrap();

    harness.inject_key(&Key::Ctrl('z')).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let process_group = i32::try_from(harness.child_pid().unwrap()).unwrap();
    assert_eq!(unsafe { libc::kill(-process_group, libc::SIGCONT) }, 0);
    harness
        .wait_for_text(
            "INTERRUPTIBLE_EDITOR_CONTINUED",
            WaitTimeout::secs(10, "editor continued after ctrl-z"),
        )
        .unwrap();

    harness.inject_key(&Key::Ctrl('c')).unwrap();
    harness
        .wait_for_raw_sequence_occurrences(
            ALTERNATE_SCREEN_ENTER,
            expected_screen_entries,
            WaitTimeout::secs(10, "TUI resumed after editor interrupt"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "draft survives editor interrupt",
            WaitTimeout::secs(10, "rho resumed after editor interrupt"),
        )
        .unwrap();
    harness.settle_input();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "fixture response: draft survives editor interrupt",
            WaitTimeout::secs(20, "preserved draft response"),
        )
        .unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn external_editor_works_during_an_active_turn() {
    use std::os::unix::fs::PermissionsExt;

    let home = IsolatedHome::new().unwrap();
    let editor_dir = tempfile::tempdir().unwrap();
    let editor = editor_dir.path().join("running-turn-editor");
    std::fs::write(
        &editor,
        r#"#!/bin/sh
printf 'RUNNING_EDITOR_READY\n'
IFS= read -r replacement
printf '%s\n' "$replacement" > "$1"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("EDITOR", editor.display().to_string());
    let mut harness = PtyHarness::spawn_named(&plan, "external_editor_during_turn").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("fixture delay").unwrap();
    harness
        .wait_for_text(
            "partial assistant before cancellation",
            WaitTimeout::secs(20, "active turn"),
        )
        .unwrap();

    harness.type_text("draft during turn").unwrap();
    let expected_screen_entries = harness.raw_sequence_occurrences(ALTERNATE_SCREEN_ENTER) + 1;
    harness.inject_key(&Key::Ctrl('g')).unwrap();
    harness
        .wait_for_text(
            "RUNNING_EDITOR_READY",
            WaitTimeout::secs(10, "running editor ready"),
        )
        .unwrap();
    harness.type_text("edited steering prompt").unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_raw_sequence_occurrences(
            ALTERNATE_SCREEN_ENTER,
            expected_screen_entries,
            WaitTimeout::secs(10, "TUI resumed during active turn"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "edited steering prompt",
            WaitTimeout::secs(10, "running editor return"),
        )
        .unwrap();
    harness.settle_input();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "queued steer 1 for after the current assistant turn",
            WaitTimeout::secs(10, "edited steering queued"),
        )
        .unwrap();

    harness.inject_key(&Key::Esc).unwrap();
    harness
        .wait_for_quiet(Duration::from_millis(250), WaitTimeout::secs(10, "cancel"))
        .unwrap();
    harness.inject_key(&Key::Ctrl('c')).unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}
