//! End-to-end Rho TUI scenarios driven through a PTY.
//!
//! These tests require a Unix PTY and a debug-built `rho` binary with the
//! fixture matrix (`RHO_TUI_TEST_MODE=matrix`).

#![cfg(unix)]

use std::path::PathBuf;

use rho_tui_pty::{all_scenarios, run_named, ScenarioRunner};

fn runner() -> ScenarioRunner {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let artifacts = std::env::temp_dir().join("rho-pty-test-artifacts");
    ScenarioRunner::new(binary).with_artifacts(artifacts)
}

fn assert_pass(name: &str) {
    let outcome = run_named(&runner(), name).expect("scenario runner error");
    assert!(
        outcome.passed,
        "scenario {name} failed:\n{}",
        outcome.message
    );
}

#[test]
fn smoke_startup_stream_exit() {
    assert_pass("startup_stream_exit");
}

#[test]
fn smoke_cancel_and_resubmit() {
    assert_pass("cancel_and_resubmit");
}

#[test]
fn smoke_resize_during_stream() {
    assert_pass("resize_during_stream");
}

#[test]
fn smoke_scroll_during_stream() {
    assert_pass("scroll_during_stream");
}

#[test]
fn smoke_terminal_restoration() {
    assert_pass("terminal_restoration");
}

#[test]
fn renders_markdown_headings() {
    assert_pass("markdown_headings");
}

#[test]
fn smoke_subset_is_registered() {
    let smoke = all_scenarios()
        .iter()
        .filter(|scenario| scenario.smoke)
        .map(|scenario| scenario.id)
        .collect::<Vec<_>>();
    assert!(smoke.contains(&"startup_stream_exit"));
    assert!(smoke.contains(&"cancel_and_resubmit"));
    assert!(smoke.contains(&"resize_during_stream"));
    assert!(smoke.contains(&"scroll_during_stream"));
    assert!(smoke.contains(&"terminal_restoration"));
}
