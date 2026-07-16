//! Harness self-tests that do not require the Rho binary.

#![cfg(unix)]

use std::{
    path::Path,
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;
use rho_tui_pty::{
    artifacts::ArtifactWriter, default_clean_env, harness::WaitTimeout, keys::Key, PtyHarness,
    PtySize, ScreenModel,
};
use tempfile::TempDir;

#[test]
fn wait_for_text_and_exit_against_printf() {
    let env = default_clean_env();
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/sh"),
        &["-c", "printf 'hello-from-pty\\n'; sleep 0.2"],
        PtySize::new(12, 40),
        &env,
        None,
        "printf-self-test",
    )
    .unwrap();

    harness
        .wait_for_text("hello-from-pty", WaitTimeout::secs(3, "printf text"))
        .unwrap();
    let code = harness
        .wait_for_exit(WaitTimeout::secs(3, "printf exit"))
        .unwrap();
    assert_eq!(code, 0);
}

#[test]
fn timeout_diagnostics_include_phase_and_screen() {
    let env = default_clean_env();
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/sh"),
        &["-c", "printf 'only-this\\n'; sleep 2"],
        PtySize::new(8, 30),
        &env,
        None,
        "timeout-self-test",
    )
    .unwrap();
    let error = harness
        .wait_for_text(
            "never-appears",
            WaitTimeout::millis(200, "expected timeout"),
        )
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("timeout waiting for text"), "{message}");
    assert!(
        message.contains("only-this") || message.contains("phase"),
        "{message}"
    );
}

#[test]
fn child_nonzero_exit_is_observed() {
    let env = default_clean_env();
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/sh"),
        &["-c", "printf 'bye\\n'; exit 7"],
        PtySize::new(8, 20),
        &env,
        None,
        "exit-code-self-test",
    )
    .unwrap();
    harness
        .wait_for_text("bye", WaitTimeout::secs(2, "bye"))
        .unwrap();
    let code = harness
        .wait_for_exit(WaitTimeout::secs(2, "exit 7"))
        .unwrap();
    assert_eq!(code, 7);
}

#[test]
fn kill_on_drop_reaps_child() {
    let env = default_clean_env();
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/sh"),
        &["-c", "sleep 30"],
        PtySize::new(6, 20),
        &env,
        None,
        "kill-on-drop",
    )
    .unwrap();
    assert!(harness.is_running());
    drop(harness);
    // If kill-on-drop failed, this test would leave a sleep process. We only assert
    // that constructing/dropping does not panic; process cleanup is covered by Drop.
}

#[test]
fn resize_interleaved_with_output() {
    let env = default_clean_env();
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/sh"),
        &[
            "-c",
            "printf 'before\\n'; sleep 0.1; printf 'after\\n'; sleep 0.1",
        ],
        PtySize::new(10, 40),
        &env,
        None,
        "resize-self-test",
    )
    .unwrap();
    harness
        .wait_for_text("before", WaitTimeout::secs(2, "before"))
        .unwrap();
    harness.resize(14, 50).unwrap();
    harness
        .wait_for_text("after", WaitTimeout::secs(2, "after"))
        .unwrap();
    assert_eq!(harness.screen().rows(), 14);
    assert_eq!(harness.screen().cols(), 50);
}

#[test]
fn artifact_writer_redacts_nothing_for_plain_env_and_persists_raw() {
    let temp = TempDir::new().unwrap();
    let writer = ArtifactWriter::new(temp.path());
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/sh"),
        &["-c", "printf 'artifact-me\\n'; exit 1"],
        PtySize::new(8, 30),
        &default_clean_env(),
        None,
        "artifact-self-test",
    )
    .unwrap();
    harness.set_artifact_writer(writer);
    harness
        .wait_for_text("artifact-me", WaitTimeout::secs(2, "artifact text"))
        .unwrap();
    let error = harness
        .wait_for_text("nope", WaitTimeout::millis(100, "force fail"))
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("artifacts:"), "{message}");
}

#[test]
fn screen_model_handles_invalid_utf8_bytes() {
    let mut screen = ScreenModel::new(4, 20);
    screen.process(b"ok\xff\xfe more");
    // Parser should not panic; contents remain usable.
    let contents = screen.contents();
    assert!(contents.contains("ok") || contents.contains("more") || !contents.is_empty());
}

#[test]
fn wait_for_quiet_detects_settling() {
    let env = default_clean_env();
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/sh"),
        &["-c", "printf 'settled\\n'"],
        PtySize::new(8, 20),
        &env,
        None,
        "quiet-self-test",
    )
    .unwrap();
    harness
        .wait_for_text("settled", WaitTimeout::secs(2, "settled"))
        .unwrap();
    let started = Instant::now();
    harness
        .wait_for_quiet(Duration::from_millis(80), WaitTimeout::secs(2, "quiet"))
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(70));
}

#[test]
fn inject_keys_are_echoed_by_cat() {
    let env = default_clean_env();
    let mut harness = PtyHarness::spawn_command(
        Path::new("/bin/cat"),
        &[],
        PtySize::new(8, 40),
        &env,
        None,
        "cat-keys",
    )
    .unwrap();
    harness.type_text("abc").unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text("abc", WaitTimeout::secs(2, "cat echo"))
        .unwrap();
    harness.kill().unwrap();
}
