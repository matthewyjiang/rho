//! Harness self-tests that do not require the Rho binary.

#![cfg(unix)]

use std::path::Path;

use pretty_assertions::assert_eq;
use rho_tui_pty::{
    artifacts::ArtifactWriter, default_clean_env, harness::WaitTimeout, keys::Key, PtyHarness,
    PtySize,
};
use tempfile::TempDir;

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
