//! End-to-end Rho TUI scenarios driven through a PTY.
//!
//! These tests require a Unix PTY and a debug-built `rho` binary with the
//! fixture matrix (`RHO_TUI_TEST_MODE=matrix`).

#![cfg(unix)]

#[path = "support/claude_e2e.rs"]
mod claude_e2e;

use std::{
    fs::{self, OpenOptions},
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
fn claude_code_login_hands_terminal_to_fake_claude() {
    let home = IsolatedHome::new().unwrap();
    // Ensure /login claude-code does not stop at credential-store choice.
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"

[behavior]
credential_store = "file"
"#,
    )
    .unwrap();

    let fake = claude_e2e::install_fake_claude_login();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("PATH", &fake.path);
    let mut harness = PtyHarness::spawn_named(&plan, "claude_code_login").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.submit_text("/login claude-code").unwrap();
    harness
        .wait_for_text(
            "handing the terminal to the claude binary",
            WaitTimeout::secs(10, "handoff notice"),
        )
        .unwrap();
    // The fake claude process exits immediately, so its stdout may only appear on
    // the suspended main screen. Prefer the post-status source of truth.
    harness
        .wait_for_text(
            "signed in as fake@example.com",
            WaitTimeout::secs(10, "post-login status"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "Managed by the claude binary",
            WaitTimeout::secs(10, "ownership copy"),
        )
        .unwrap();
    assert!(fake.marker.exists(), "fake claude login should have run");
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn login_shows_provider_picker_before_credential_store_choice() {
    let home = IsolatedHome::new().unwrap();
    // Deliberately leave behavior.credential_store unset so first normal
    // provider login must choose a store after the group picker.
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
    let mut harness = PtyHarness::spawn_named(&plan, "login_provider_then_store").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Bare /login opens the group picker first, not the store chooser.
    harness.submit_text("/login").unwrap();
    harness
        .wait_for_text(
            "select provider to login",
            WaitTimeout::secs(10, "group picker first"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        !screen.contains("Where should Rho store provider credentials?"),
        "store chooser must wait until a normal provider is selected:\n{screen}"
    );
    assert!(
        !screen.contains("Claude Code (delegation only)"),
        "claude-code belongs under Anthropic methods, not the top-level group picker:\n{screen}"
    );
    assert!(
        screen.contains("Anthropic"),
        "Anthropic group must remain in the bare login picker:\n{screen}"
    );

    // Filter to OpenAI so the test does not depend on picker sort order.
    harness.type_text("openai").unwrap();
    harness
        .wait_for_text("OpenAI", WaitTimeout::secs(5, "openai filtered"))
        .unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "select OpenAI login method",
            WaitTimeout::secs(10, "openai methods"),
        )
        .unwrap();
    // API Key is the first method.
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "Where should Rho store provider credentials?",
            WaitTimeout::secs(10, "store after provider"),
        )
        .unwrap();
    harness
        .wait_for_text("Local file", WaitTimeout::secs(5, "file option"))
        .unwrap();
    harness.inject_key(&Key::Esc).unwrap();
    harness
        .wait_for_quiet(
            Duration::from_millis(150),
            WaitTimeout::secs(5, "after cancel"),
        )
        .unwrap();

    // Direct provider args still prompt for the store before secrets.
    harness.submit_text("/login openai").unwrap();
    harness
        .wait_for_text(
            "Where should Rho store provider credentials?",
            WaitTimeout::secs(10, "store for direct provider"),
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
        .wait_for_text("enter", WaitTimeout::secs(10, "api key prompt"))
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
    let mut harness = PtyHarness::spawn_named(&plan, "login_provider_then_store_again").unwrap();
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
fn login_claude_code_skips_credential_store_when_unset() {
    let home = IsolatedHome::new().unwrap();
    // Leave credential_store unset. Claude login must never ask for it.
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

    let fake = claude_e2e::install_fake_claude_login();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("PATH", &fake.path);
    let mut harness = PtyHarness::spawn_named(&plan, "claude_code_login_no_store").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.submit_text("/login claude-code").unwrap();
    harness
        .wait_for_text(
            "handing the terminal to the claude binary",
            WaitTimeout::secs(10, "handoff notice"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        !screen.contains("Where should Rho store provider credentials?"),
        "claude-code must never open the Rho store chooser:\n{screen}"
    );
    harness
        .wait_for_text(
            "signed in as fake@example.com",
            WaitTimeout::secs(10, "post-login status"),
        )
        .unwrap();
    assert!(fake.marker.exists(), "fake claude login should have run");
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    let config = std::fs::read_to_string(&home.config_path).unwrap();
    assert!(
        !config.contains("credential_store"),
        "claude login must not write Rho credential_store:\n{config}"
    );
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
fn goal_answers_background_agent_questionnaire_while_waiting() {
    assert_pass("goal_questionnaire");
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
fn subagent_rail_mouse_activation_uses_release_and_survives_refresh() {
    assert_pass("subagent_rail_mouse");
}

#[test]
fn background_agent_questionnaire_is_answered_in_parent_tui() {
    assert_pass("background_agent_questionnaire");
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
        .wait_for_text(
            "attached to c1a0de",
            WaitTimeout::secs(10, "attach startup"),
        )
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

/// Full fake-Claude runtime path: matrix parent -> agent tool -> binder/executor
/// -> `claude -p` spawn -> stream-json -> result/events persistence -> parent
/// completion UI -> `rho attach` replay. Never touches a real Claude binary or
/// the network.
#[test]
fn fake_claude_runtime_end_to_end_success() {
    let home = IsolatedHome::new().unwrap();
    claude_e2e::install_claude_planner_agent(&home.home);

    let fake_root = home.path().join("fake-claude");
    let fake = claude_e2e::install_fake_claude(&fake_root, claude_e2e::FakeClaudeMode::Success);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);

    // Prove the isolated PATH cannot resolve a host Claude: only our stub.
    assert!(fake.claude.is_file());
    assert_eq!(
        which_on_path("claude", &path).as_deref(),
        Some(fake.claude.as_path()),
        "PATH must resolve the fake claude first"
    );

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_runtime_e2e").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Real agent-tool path via matrix fixture prompt (foreground delegation).
    harness.submit_text("fixture claude agent").unwrap();
    harness
        .wait_for_text(
            "claude-planner",
            WaitTimeout::secs(15, "agent tool started"),
        )
        .unwrap();

    claude_e2e::wait_for_spawn(&fake, Duration::from_secs(15));
    let record = fake.read_spawn_record();
    claude_e2e::assert_success_spawn(&record, &home.workspace);

    // Parent UI shows final text and Claude session id from the live fixture.
    harness
        .wait_for_text(
            "rho-claude-e2e-ok",
            WaitTimeout::secs(20, "final assistant text"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "11111111-2222-4333-8444-555555555555",
            WaitTimeout::secs(10, "claude session id"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "claude agent tool finished:",
            WaitTimeout::secs(15, "parent turn closed"),
        )
        .unwrap();

    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let run_id = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .expect("run id")
        .to_string();
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    claude_e2e::assert_success_result(&status, &run_dir);
    assert!(
        run_dir.starts_with(home.home.join(".rho/sessions")),
        "interactive run was not nested under its session: {}",
        run_dir.display()
    );
    assert!(
        !home.home.join(".rho/subagents").join(&run_id).exists(),
        "interactive run also appeared in the global pool"
    );

    // Offline proof: only the fake binary ran; spawn marker is under the temp root.
    assert!(
        fake.spawn_marker.starts_with(home.path()),
        "spawn marker escaped isolated root: {}",
        fake.spawn_marker.display()
    );
    assert!(
        record.args.iter().all(|arg| !arg.contains("anthropic.com")),
        "spawn argv must not reference network endpoints: {:?}",
        record.args
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    // Replay the real on-disk artifacts through `rho attach`.
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let mut plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 120,
        },
    )
    .with_arg("attach")
    .with_arg(&run_id);
    plan.cwd = home.path().join("attach-workspace");
    fs::create_dir_all(&plan.cwd).unwrap();
    let mut attach = PtyHarness::spawn_named(&plan, "fake_claude_runtime_e2e_attach").unwrap();
    attach
        .wait_for_text(
            &format!("attached to {run_id}"),
            WaitTimeout::secs(10, "attach startup"),
        )
        .unwrap();
    attach
        .wait_for_text(
            "Say hello in one short sentence.",
            WaitTimeout::secs(5, "attach prompt"),
        )
        .unwrap();
    // Partial deltas are stored separately; the live fixture splits the final
    // phrase across two assistant_text_delta events ("r" + "ho-claude-e2e-ok").
    attach
        .wait_for_text(
            "ho-claude-e2e-ok",
            WaitTimeout::secs(5, "attach final text tail"),
        )
        .unwrap();
    attach
        .wait_for_text(
            "claude 11111111-2222-4333-8444-555555555555",
            WaitTimeout::secs(5, "attach session id"),
        )
        .unwrap();
    attach
        .wait_for_text("claude-planner", WaitTimeout::secs(5, "attach agent id"))
        .unwrap();
    assert!(attach.screen().contains_text("read-only"));
    assert!(attach.screen().contains_text("ok") || attach.screen().contains_text("complete"));
    // Joined final text lives in result.json (asserted earlier); attach stream
    // fidelity keeps the partial pieces visible.
    let attach_screen = attach.screen().contents();
    assert!(
        attach_screen.contains('r') && attach_screen.contains("ho-claude-e2e-ok"),
        "attach should replay streamed halves:\n{attach_screen}"
    );
    attach.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(
        attach
            .wait_for_exit(WaitTimeout::secs(5, "detach"))
            .unwrap(),
        0
    );
}

/// Background Claude completion path: terminal `total_cost_usd` must fold into
/// the parent session total shown by `/info` after automatic delivery.
#[test]
fn fake_claude_background_cost_appears_in_info() {
    let home = IsolatedHome::new().unwrap();
    claude_e2e::install_claude_planner_agent(&home.home);

    let fake_root = home.path().join("fake-claude");
    let fake = claude_e2e::install_fake_claude(&fake_root, claude_e2e::FakeClaudeMode::Success);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 36,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_background_cost_info").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness
        .submit_text("fixture background claude agent")
        .unwrap();
    harness
        .wait_for_text(
            "background claude agent dispatched:",
            WaitTimeout::secs(15, "background dispatch"),
        )
        .unwrap();

    claude_e2e::wait_for_spawn(&fake, Duration::from_secs(15));
    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    claude_e2e::assert_success_result(&status, &run_dir);

    harness
        .wait_for_text(
            "background claude agent completion received with delegated result (delivery 1)",
            WaitTimeout::secs(20, "completion delivery"),
        )
        .unwrap();

    // Delivery should include the fixture cost on the statusline total.
    harness
        .wait_for_text("$0.034", WaitTimeout::secs(10, "statusline subagent cost"))
        .unwrap();

    harness.submit_text("/info").unwrap();
    harness
        .wait_for_text("Session usage", WaitTimeout::secs(10, "info opened"))
        .unwrap();
    harness
        .wait_for_text(
            "Subagent cost",
            WaitTimeout::secs(10, "subagent cost label"),
        )
        .unwrap();
    harness
        .wait_for_text("$0.034", WaitTimeout::secs(10, "subagent cost value"))
        .unwrap();

    let screen = harness.screen().contents();
    assert!(
        screen.contains("Subagent cost") && screen.contains("$0.034"),
        "expected /info to show delivered subagent cost:\n{screen}"
    );
    assert!(
        !screen.contains("No token usage recorded yet."),
        "subagent cost should replace the empty-usage note:\n{screen}"
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Sibling error path: fake Claude emits a terminal error stream and nonzero
/// exit; the parent surfaces a failed delegated run without network access.
#[test]
fn fake_claude_runtime_end_to_end_error() {
    let home = IsolatedHome::new().unwrap();
    claude_e2e::install_claude_planner_agent(&home.home);

    let fake_root = home.path().join("fake-claude");
    let fake = claude_e2e::install_fake_claude(&fake_root, claude_e2e::FakeClaudeMode::Error);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_runtime_e2e_error").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("fixture claude agent error").unwrap();
    harness
        .wait_for_text(
            "claude-planner",
            WaitTimeout::secs(15, "agent tool started"),
        )
        .unwrap();

    claude_e2e::wait_for_spawn(&fake, Duration::from_secs(15));

    // Failed foreground agent should show failed presentation and/or error text.
    harness
        .wait_for_text("failed", WaitTimeout::secs(20, "failed agent state"))
        .unwrap();

    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    claude_e2e::assert_error_result(&status);
    assert!(
        fake.spawn_marker.exists(),
        "error path must still have spawned the fake binary"
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Resolve `program` on a PATH string the same way a shell would (first hit).
fn which_on_path(program: &str, path_var: &str) -> Option<PathBuf> {
    for dir in path_var.split(':').filter(|dir| !dir.is_empty()) {
        let candidate = PathBuf::from(dir).join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
