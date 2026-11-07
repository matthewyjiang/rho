//! Handler execution against real child processes.
//!
//! These stay Unix-only because they need a shell script to model the failure
//! modes that matter (background descendants, partial stdin reads, huge output).
//! The Windows job-object path is exercised by the shared shell-tool suite.
#![cfg(unix)]

use std::{collections::BTreeMap, ffi::OsString, path::Path, sync::Arc, time::Duration};

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
    run_hook(
        hook,
        event,
        &Ambient::ThisProcess,
        rho_sdk::CancellationToken::new(),
    )
    .await
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

// Covers: a hook that outlives its timeout fails with `TimedOut`, and its
// background descendants die with it instead of leaking.
// Owner: hook process supervision.
#[tokio::test]
async fn a_timed_out_handler_takes_its_descendants_with_it() {
    let home = TempDir::new().unwrap();
    let marker = home.path().join("descendant.pid");
    // `$!` is the background sleep itself. `$$` would name the hook's own
    // shell, which dies with the direct child even when the group survives.
    let hook = hook_running(
        &format!("sleep 60 & echo $! > {}; wait", marker.display()),
        "1s",
        &[],
        &home,
    );

    let error = run(&hook, "{}").await.unwrap_err();
    assert_eq!(
        error,
        HookRunError::TimedOut {
            timeout: Duration::from_secs(1)
        }
    );

    let pid = wait_for_pid(&marker).await;
    // The group kill is asynchronous; bound the wait instead of guessing it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::process_tree::process_is_running(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "descendant {pid} outlived its hook timeout"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
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

    let error = run_hook(&hook, "{}", &Ambient::ThisProcess, cancellation)
        .await
        .unwrap_err();

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

// Covers: a hook starts with exactly its session's base set and allowlist,
// read from the session's environment, such as a daemon client's, and
// nothing else of it (#1659). `child_environment` owns which names pass
// through; this owns that the command gets exactly that map from the session.
// Owner: hook spawn
#[test]
fn a_handler_gets_exactly_its_sessions_base_set_and_allowlist() {
    let home = TempDir::new().unwrap();
    let hook = hook_running("true", "10s", &["MY_HOOK_TOKEN"], &home);
    let session = Ambient::Client(Arc::new(BTreeMap::from(
        [
            ("PATH", "/client/bin"),
            ("HOME", "/home/client"),
            ("MY_HOOK_TOKEN", "token"),
            ("OPENAI_API_KEY", "secret"),
            ("UNRELATED", "value"),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned())),
    )));

    let command = build_command(&hook, &session);

    let environment = command
        .as_std()
        .get_envs()
        .map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned)))
        .collect::<BTreeMap<_, _>>();
    let expected = [
        ("PATH", "/client/bin"),
        ("HOME", "/home/client"),
        ("MY_HOOK_TOKEN", "token"),
        (crate::hooks::IN_HOOK_ENV, "1"),
    ]
    .map(|(name, value)| (OsString::from(name), Some(OsString::from(value))));
    assert_eq!(environment, BTreeMap::from(expected));
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
