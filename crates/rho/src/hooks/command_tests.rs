//! Handler execution against real child processes.
//!
//! These stay Unix-only because they need a shell script to model the failure
//! modes that matter (background descendants, partial stdin reads, huge output).
//! The Windows job-object path is exercised by the shared shell-tool suite.
#![cfg(unix)]

use std::{path::Path, time::Duration};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::hooks::{
    catalog::{HookCatalog, ProjectTrust},
    config::HookDefinition,
};

/// Writes an executable script and returns a hook that runs it.
fn hook_running(script: &str, timeout: &str, env: &[&str], home: &TempDir) -> HookDefinition {
    let program = home.path().join("handler.sh");
    std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();

    let env_list = env
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        home.path().join("hooks.toml"),
        format!(
            "version = 1\n\n[[hook]]\nid = \"handler\"\non = \"after_tool_use\"\ncommand = [\"/bin/sh\", \"{}\"]\ntimeout = \"{timeout}\"\nenv = [{env_list}]\n",
            program.display()
        ),
    )
    .unwrap();
    HookCatalog::discover(Some(home.path()), None, ProjectTrust::Untrusted)
        .unwrap()
        .hooks()[0]
        .clone()
}

async fn run(hook: &HookDefinition, event: &str) -> Result<HookRunOutput, HookRunError> {
    run_hook(hook, event, rho_sdk::CancellationToken::new()).await
}

#[tokio::test]
async fn the_event_arrives_on_stdin_and_stdout_comes_back() {
    let home = TempDir::new().unwrap();
    let hook = hook_running("cat", "10s", &[], &home);

    let output = run(&hook, r#"{"event":"after_tool_use"}"#).await.unwrap();

    assert!(output.succeeded());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        r#"{"event":"after_tool_use"}"#
    );
}

#[tokio::test]
async fn a_handler_that_never_reads_stdin_still_completes() {
    let home = TempDir::new().unwrap();
    let hook = hook_running("echo ok", "10s", &[], &home);

    let output = run(&hook, &"x".repeat(200_000)).await.unwrap();

    assert!(output.succeeded());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
}

#[tokio::test]
async fn a_nonzero_exit_is_reported_with_its_code_and_stderr() {
    let home = TempDir::new().unwrap();
    let hook = hook_running("echo boom >&2; exit 3", "10s", &[], &home);

    let output = run(&hook, "{}").await.unwrap();

    assert!(!output.succeeded());
    assert_eq!(output.exit_code, Some(3));
    assert_eq!(output.stderr_summary(), "boom");
}

#[tokio::test]
async fn a_slow_handler_times_out() {
    let home = TempDir::new().unwrap();
    let hook = hook_running("sleep 30", "1s", &[], &home);

    let error = run(&hook, "{}").await.unwrap_err();

    assert_eq!(
        error,
        HookRunError::TimedOut {
            timeout: Duration::from_secs(1)
        }
    );
}

#[tokio::test]
async fn a_timed_out_handler_takes_its_descendants_with_it() {
    let home = TempDir::new().unwrap();
    let marker = home.path().join("descendant.pid");
    let hook = hook_running(
        &format!("( echo $$ > {}; sleep 60 ) & sleep 30", marker.display()),
        "1s",
        &[],
        &home,
    );

    let error = run(&hook, "{}").await.unwrap_err();
    assert!(matches!(error, HookRunError::TimedOut { .. }));

    let pid = wait_for_pid(&marker).await;
    // Give the kernel a moment to reap the group before probing it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !process_is_alive(pid),
        "descendant {pid} outlived its hook timeout"
    );
}

#[tokio::test]
async fn cancellation_stops_a_running_handler() {
    let home = TempDir::new().unwrap();
    let hook = hook_running("sleep 30", "60s", &[], &home);
    let cancellation = rho_sdk::CancellationToken::new();
    let canceller = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        canceller.cancel();
    });

    let error = run_hook(&hook, "{}", cancellation).await.unwrap_err();

    assert_eq!(error, HookRunError::Cancelled);
}

#[tokio::test]
async fn a_missing_program_reports_a_spawn_failure() {
    let home = TempDir::new().unwrap();
    let mut hook = hook_running("true", "10s", &[], &home);
    hook.executable = home.path().join("gone");
    hook.command = vec![crate::paths::display(&hook.executable)];

    let error = run(&hook, "{}").await.unwrap_err();

    assert!(matches!(error, HookRunError::Spawn(_)));
}

#[tokio::test]
async fn output_larger_than_the_bound_is_captured_up_to_the_bound_and_flagged() {
    let home = TempDir::new().unwrap();
    let hook = hook_running(
        "head -c 200000 /dev/zero | tr '\\0' 'a'; head -c 100000 /dev/zero | tr '\\0' 'e' >&2",
        "30s",
        &[],
        &home,
    );

    let output = run(&hook, "{}").await.unwrap();

    assert!(output.truncated);
    assert!(output.stdout.len() <= crate::hooks::protocol::MAX_DECISION_BYTES + 1);
    assert!(output.stderr.len() <= crate::cli_runtime::MAX_STDERR_BYTES + rho_sdk::ELLIPSIS.len());
    assert!(output.stderr.starts_with(rho_sdk::ELLIPSIS));
}

#[tokio::test]
async fn invalid_utf8_output_is_captured_without_panicking() {
    let home = TempDir::new().unwrap();
    let hook = hook_running(r"printf 'ok\377'", "10s", &[], &home);

    let output = run(&hook, "{}").await.unwrap();

    assert!(output.succeeded());
    assert_eq!(output.stdout, b"ok\xff");
}

#[tokio::test]
async fn a_handler_receives_only_the_environment_the_contract_promises() {
    let home = TempDir::new().unwrap();
    // `child_environment` owns which names pass through; this owns whether the
    // spawned child really gets exactly that map. Reading an ambient name the
    // contract excludes avoids mutating the shared process environment.
    let excluded = std::env::vars()
        .map(|(name, _)| name)
        .find(|name| {
            !super::super::environment::base_environment_names().contains(&name.as_str())
                && name != crate::hooks::IN_HOOK_ENV
        })
        .expect("the test process has at least one variable outside the base set");
    let hook = hook_running("env | sort", "10s", &[], &home);

    let output = run(&hook, "{}").await.unwrap();

    let environment = String::from_utf8_lossy(&output.stdout);
    assert!(environment.contains(&format!("{}=1", crate::hooks::IN_HOOK_ENV)));
    assert!(
        !environment.contains(&format!("{excluded}=")),
        "ambient variable {excluded} reached a hook: {environment}"
    );
}

#[tokio::test]
async fn a_handler_runs_in_its_configured_working_directory() {
    let home = TempDir::new().unwrap();
    let hook = hook_running("pwd", "10s", &[], &home);

    let output = run(&hook, "{}").await.unwrap();

    assert_eq!(
        std::fs::canonicalize(String::from_utf8_lossy(&output.stdout).trim()).unwrap(),
        std::fs::canonicalize(hook.working_directory()).unwrap()
    );
}

async fn wait_for_pid(path: &Path) -> i32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse::<i32>() {
                return pid;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the descendant never reported its PID"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn process_is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}
