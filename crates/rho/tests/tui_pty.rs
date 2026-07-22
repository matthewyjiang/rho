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
fn login_prompts_for_credential_store_before_provider_picker() {
    let home = IsolatedHome::new().unwrap();
    // Deliberately leave behavior.credential_store unset so first /login must choose.
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"
"#,
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    );
    let mut harness =
        PtyHarness::spawn_named(&plan, "login_credential_store_choice").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Cancel path: chooser appears, esc returns to the prompt.
    harness.submit_text("/login").unwrap();
    harness
        .wait_for_text(
            "Where should Rho store provider credentials?",
            WaitTimeout::secs(10, "chooser open"),
        )
        .unwrap();
    harness
        .wait_for_text("Local file", WaitTimeout::secs(5, "file option"))
        .unwrap();
    harness.inject_key(&Key::Esc).unwrap();
    harness
        .wait_for_quiet(Duration::from_millis(150), WaitTimeout::secs(5, "after cancel"))
        .unwrap();

    // Choose file via stable shortcut, then land on the provider picker.
    harness.submit_text("/login").unwrap();
    harness
        .wait_for_text(
            "Where should Rho store provider credentials?",
            WaitTimeout::secs(10, "chooser reopen"),
        )
        .unwrap();
    harness.inject_key(&Key::Char('2')).unwrap();
    harness
        .wait_for_text(
            "credential store set to file",
            WaitTimeout::secs(10, "store persisted"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "select provider to login",
            WaitTimeout::secs(10, "provider picker"),
        )
        .unwrap();
    harness.inject_key(&Key::Esc).unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    let config = std::fs::read_to_string(&home.config_path).unwrap();
    assert!(
        config.contains("credential_store = \"file\""),
        "chooser should persist file backend:\n{config}"
    );

    // Second /login must not re-prompt once config is set.
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    );
    let mut harness =
        PtyHarness::spawn_named(&plan, "login_credential_store_choice_again").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup again"))
        .unwrap();
    harness.submit_text("/login").unwrap();
    harness
        .wait_for_text(
            "select provider to login",
            WaitTimeout::secs(10, "no second chooser"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        !screen.contains("Where should Rho store provider credentials?"),
        "chooser should not reappear after config is set:\n{screen}"
    );
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn model_command_resolves_configured_alias() {
    let home = IsolatedHome::new().unwrap();
    std::fs::write(
        &home.config_path,
        r#"check_for_updates = false
web_search_provider = "disabled"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[model.aliases]
deep = "openai-codex/gpt-5.5"
"#,
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    );
    let mut harness = PtyHarness::spawn_named(&plan, "resolve_model_alias").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("/model @deep").unwrap();
    harness
        .wait_for_text(
            "model switched to openai-codex/gpt-5.5",
            WaitTimeout::secs(10, "model switch"),
        )
        .unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    let config = std::fs::read_to_string(&home.config_path).unwrap();
    assert!(
        config.contains("model = \"@deep\""),
        "saved config:\n{config}"
    );
}

#[test]
fn runtime_info_reflows_after_narrow_resize() {
    assert_pass("runtime_info");
}

#[test]
fn renders_markdown_headings() {
    assert_pass("markdown_headings");
}

#[test]
fn bare_skill_command_starts_a_model_turn() {
    let home = IsolatedHome::new().unwrap();
    let skill_dir = home.workspace.join(".agents/skills/test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: Test skill invocation\ndisable-model-invocation: true\n---\nFollow the unique bare skill instruction.\n",
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    );
    let mut harness = PtyHarness::spawn_named(&plan, "bare_skill_command").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("/skill:test-skill").unwrap();
    harness
        .wait_for_text(
            "skill command loaded before model response: Follow the unique bare skill instruction.",
            WaitTimeout::secs(20, "skill response"),
        )
        .unwrap();

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn skill_command_reports_when_skill_tool_is_disabled() {
    let home = IsolatedHome::new().unwrap();
    let skill_dir = home.workspace.join(".agents/skills/test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: Test skill invocation\n---\nFollow the skill.\n",
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_arg("--no-tools");
    let mut harness = PtyHarness::spawn_named(&plan, "disabled_skill_command").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("/skill:test-skill").unwrap();
    harness
        .wait_for_text(
            "skill commands are unavailable because the active agent has no skill tool",
            WaitTimeout::secs(5, "skill unavailable"),
        )
        .unwrap();

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn goal_waits_for_subagents_before_evaluation() {
    assert_pass("goal_waits_for_subagents");
}

#[test]
fn goal_waits_for_subagents_before_retrying() {
    assert_pass("goal_waits_for_subagents_during_retry");
}

#[test]
fn background_agent_completion_is_delivered_after_turn_end() {
    assert_pass("background_agent_auto_delivery");
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
