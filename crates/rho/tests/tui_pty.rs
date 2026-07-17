//! End-to-end Rho TUI scenarios driven through a PTY.
//!
//! These tests require a Unix PTY and a debug-built `rho` binary with the
//! fixture matrix (`RHO_TUI_TEST_MODE=matrix`).

#![cfg(unix)]

use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use rho_tui_pty::{
    all_scenarios, run_named, IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan,
    ScenarioRunner, WaitTimeout,
};

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
fn smoke_type_during_stream() {
    assert_pass("type_during_stream");
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
fn attach_is_read_only_and_updates_live() {
    let home = IsolatedHome::new().unwrap();
    let directory = home.home.join(".rho/subagents/abc123");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("result.json"),
        r#"{
            "state": "running",
            "agent_id": "explorer",
            "turns": 1,
            "input_tokens": 12,
            "output_tokens": 3,
            "last_activity": "assistant text"
        }"#,
    )
    .unwrap();
    let events = directory.join("events.jsonl");
    std::fs::write(
        &events,
        "{\"type\":\"prompt\",\"data\":\"delegated task\"}\n",
    )
    .unwrap();
    let socket = home.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = Arc::clone(&requests);
    let server = std::thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            server_requests
                .lock()
                .unwrap()
                .push(serde_json::from_str::<serde_json::Value>(&line).unwrap());
            stream.write_all(b"{}\n").unwrap();
        }
    });
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(binary, &home, PtySize { rows: 24, cols: 90 })
        .with_arg("attach")
        .with_arg("abc123")
        .with_env("HERDR_ENV", "1")
        .with_env("HERDR_SOCKET_PATH", socket.display().to_string())
        .with_env("HERDR_PANE_ID", "%attach");
    let mut harness = PtyHarness::spawn_named(&plan, "attach_read_only").unwrap();

    harness
        .wait_for_text(
            "attached to abc123",
            WaitTimeout::secs(10, "attach startup"),
        )
        .unwrap();
    harness
        .wait_for_text("delegated task", WaitTimeout::secs(5, "delegated prompt"))
        .unwrap();
    harness.type_text("must not become a prompt").unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_quiet(
            Duration::from_millis(200),
            WaitTimeout::secs(5, "ignored input"),
        )
        .unwrap();
    assert!(!harness.screen().contains_text("must not become a prompt"));

    let mut file = OpenOptions::new().append(true).open(&events).unwrap();
    writeln!(
        file,
        "{{\"type\":\"assistant_text_delta\",\"data\":\"watchable answer\"}}"
    )
    .unwrap();
    file.flush().unwrap();
    harness
        .wait_for_text("watchable answer", WaitTimeout::secs(5, "live event"))
        .unwrap();
    assert!(harness.screen().contains_text("read-only"));
    std::fs::write(
        directory.join("result.json"),
        r#"{
            "state": "ok",
            "agent_id": "explorer",
            "turns": 1,
            "input_tokens": 12,
            "output_tokens": 3,
            "last_activity": "complete",
            "result": "watchable answer"
        }"#,
    )
    .unwrap();
    harness
        .wait_for_text(
            "explorer  |  complete",
            WaitTimeout::secs(5, "completion state"),
        )
        .unwrap();

    harness.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(
        harness
            .wait_for_exit(WaitTimeout::secs(5, "detach"))
            .unwrap(),
        0
    );
    assert!(String::from_utf8_lossy(harness.raw_output()).contains("?1049l"));
    server.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests[0]["method"], "pane.report_agent");
    assert_eq!(requests[0]["params"]["state"], "working");
    assert_eq!(requests[0]["params"]["agent_session_id"], "abc123");
    assert_eq!(requests[1]["method"], "pane.report_agent");
    assert_eq!(requests[1]["params"]["state"], "working");
    assert_eq!(requests[2]["method"], "pane.report_agent");
    assert_eq!(requests[2]["params"]["state"], "idle");
    assert_eq!(requests[3]["method"], "pane.release_agent");
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
    assert!(smoke.contains(&"type_during_stream"));
    assert!(smoke.contains(&"resize_during_stream"));
    assert!(smoke.contains(&"scroll_during_stream"));
    assert!(smoke.contains(&"terminal_restoration"));
}
