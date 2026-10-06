//! Read-only attachment, live event replay, and elapsed-time updates.

use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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
            "provider": "openai",
            "model": "gpt-5.5",
            "runtime": "rho",
            "started_at": 1700000000,
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
        .wait_for_text("attach abc123", WaitTimeout::secs(10, "attach startup"))
        .unwrap();
    harness
        .wait_for_text(
            "openai/gpt-5.5 · rho",
            WaitTimeout::secs(5, "provider model runtime"),
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
            "provider": "openai",
            "model": "gpt-5.5",
            "runtime": "rho",
            "started_at": 1700000000,
            "finished_at": 1700000065,
            "turns": 1,
            "input_tokens": 12,
            "output_tokens": 3,
            "last_activity": "complete",
            "result": "watchable answer"
        }"#,
    )
    .unwrap();
    harness
        .wait_for_text("complete", WaitTimeout::secs(5, "completion activity"))
        .unwrap();
    harness
        .wait_for_text("1m 05s", WaitTimeout::secs(5, "finished elapsed"))
        .unwrap();
    assert!(harness.screen().contains_text("explorer"));
    assert!(harness.screen().contains_text("ok"));

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
fn attach_live_elapsed_advances_without_status_change() {
    let home = IsolatedHome::new().unwrap();
    let directory = home.home.join(".rho/subagents/e1a95e");
    std::fs::create_dir_all(&directory).unwrap();
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(2);
    std::fs::write(
        directory.join("result.json"),
        format!(
            r#"{{
            "state": "running",
            "agent_id": "explorer",
            "provider": "openai",
            "model": "gpt-5.5",
            "runtime": "rho",
            "started_at": {started_at},
            "turns": 1,
            "last_activity": "assistant text"
        }}"#
        ),
    )
    .unwrap();
    std::fs::write(
        directory.join("events.jsonl"),
        "{\"type\":\"prompt\",\"data\":\"elapsed clock\"}\n",
    )
    .unwrap();

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(binary, &home, PtySize { rows: 24, cols: 90 })
        .with_arg("attach")
        .with_arg("e1a95e");
    let mut harness = PtyHarness::spawn_named(&plan, "attach_live_elapsed").unwrap();

    harness
        .wait_for_text("attach e1a95e", WaitTimeout::secs(10, "attach startup"))
        .unwrap();
    harness
        .wait_for_text("elapsed clock", WaitTimeout::secs(5, "prompt"))
        .unwrap();

    // Capture an initial whole-second elapsed label from the identity row.
    let first = wait_for_turn_elapsed_secs(&mut harness, WaitTimeout::secs(5, "first elapsed"));
    // result.json stays unchanged; elapsed must still tick forward.
    let later = wait_for_turn_elapsed_secs_at_least(
        &mut harness,
        first + 2,
        WaitTimeout::secs(8, "elapsed advanced"),
    );
    assert!(
        later >= first + 2,
        "elapsed should advance without status I/O (first={first}s later={later}s)"
    );

    harness.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(
        harness
            .wait_for_exit(WaitTimeout::secs(5, "detach"))
            .unwrap(),
        0
    );
}

/// Parse `turn N · Xs` whole-second elapsed from the attach identity line.
fn turn_elapsed_secs(screen: &str) -> Option<u64> {
    let marker = "turn ";
    let rest = screen.split(marker).nth(1)?;
    let after_turn = rest.split_once('·')?.1.trim_start();
    let token = after_turn.split_whitespace().next()?;
    token.strip_suffix('s')?.parse().ok()
}

fn wait_for_turn_elapsed_secs(harness: &mut PtyHarness, timeout: WaitTimeout) -> u64 {
    let started = Instant::now();
    let deadline = started + timeout.duration;
    loop {
        harness.poll(Duration::from_millis(50));
        if let Some(secs) = turn_elapsed_secs(&harness.screen().contents()) {
            return secs;
        }
        if Instant::now() >= deadline {
            panic!(
                "timeout waiting for live elapsed seconds during {}: screen=\n{}",
                timeout.label,
                harness.screen().contents()
            );
        }
    }
}

fn wait_for_turn_elapsed_secs_at_least(
    harness: &mut PtyHarness,
    minimum: u64,
    timeout: WaitTimeout,
) -> u64 {
    let started = Instant::now();
    let deadline = started + timeout.duration;
    loop {
        harness.poll(Duration::from_millis(50));
        if let Some(secs) = turn_elapsed_secs(&harness.screen().contents()) {
            if secs >= minimum {
                return secs;
            }
        }
        if Instant::now() >= deadline {
            panic!(
                "timeout waiting for elapsed >= {minimum}s during {}: screen=\n{}",
                timeout.label,
                harness.screen().contents()
            );
        }
    }
}

#[test]
fn attach_replays_finished_claude_run_from_fixtures() {
    let home = IsolatedHome::new().unwrap();
    let directory = home.home.join(".rho/subagents/c1a0de");
    std::fs::create_dir_all(&directory).unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_attach");
    std::fs::copy(fixtures.join("result.json"), directory.join("result.json")).unwrap();
    std::fs::copy(
        fixtures.join("events.jsonl"),
        directory.join("events.jsonl"),
    )
    .unwrap();

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 24,
            cols: 100,
        },
    )
    .with_arg("attach")
    .with_arg("c1a0de");
    let mut harness = PtyHarness::spawn_named(&plan, "claude_attach_replay").unwrap();

    harness
        .wait_for_text("attach c1a0de", WaitTimeout::secs(10, "attach startup"))
        .unwrap();
    harness
        .wait_for_text(
            "Say hello in one short sentence.",
            WaitTimeout::secs(5, "prompt"),
        )
        .unwrap();
    harness
        .wait_for_text("Hello from Claude.", WaitTimeout::secs(5, "assistant text"))
        .unwrap();
    harness
        .wait_for_text(
            "claude-code/claude-opus-demo · claude-cli · turn 1 · 42s",
            WaitTimeout::secs(5, "provider model runtime elapsed"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "claude sess-success-001",
            WaitTimeout::secs(5, "session id"),
        )
        .unwrap();
    harness
        .wait_for_text("claude-planner", WaitTimeout::secs(5, "agent id"))
        .unwrap();
    assert!(harness.screen().contains_text("read-only"));
    assert!(harness.screen().contains_text("ok"));

    harness.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(
        harness
            .wait_for_exit(WaitTimeout::secs(5, "detach"))
            .unwrap(),
        0
    );
}
