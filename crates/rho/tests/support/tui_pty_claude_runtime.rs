//! Fake Claude Code (`claude-agent-acp`) delegation, delivery, cost
//! accounting, and advisor flow.

use pretty_assertions::assert_eq;
use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Duration};

use super::{
    acp_e2e::{self, FakeAcpPaths},
    attach_view,
    claude_e2e::FAKE_CLAUDE,
};

/// Install the planner agent and the fake adapter playing `script`, and
/// return a PATH that resolves the fake first.
fn install_fake(home: &IsolatedHome, script: &str) -> (FakeAcpPaths, String) {
    FAKE_CLAUDE.install_agent(&home.home, "claude-planner.md");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let fake = FAKE_CLAUDE.install_script(&home.path().join("fake-claude"), &binary, script);
    let path = acp_e2e::path_with_fake(&fake.bin_dir);
    assert_eq!(
        acp_e2e::which_on_path("claude-agent-acp", &path).as_deref(),
        Some(fake.program.as_path()),
        "PATH must resolve the fake claude-agent-acp first"
    );
    (fake, path)
}

/// Where the TUI runs the session.
#[derive(Clone, Copy)]
enum Host {
    InProcess,
    Daemon,
}

fn launch(home: &IsolatedHome, path: String, rows: u16, name: &str) -> PtyHarness {
    launch_on(Host::InProcess, home, path, rows, name)
}

fn launch_on(host: Host, home: &IsolatedHome, path: String, rows: u16, name: &str) -> PtyHarness {
    let mut plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        home,
        PtySize { rows, cols: 120 },
    )
    .with_env("PATH", path);
    match host {
        Host::InProcess => {}
        Host::Daemon => plan = plan.with_env("RHO_TUI_DAEMON", "1"),
    }
    let mut harness = PtyHarness::spawn_named(&plan, name).unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness
}

/// The requests the scripted agent received, in order.
fn requests(fake: &FakeAcpPaths) -> Vec<Value> {
    fake.journal()
        .into_iter()
        .filter_map(|entry| entry.get("request").cloned())
        .collect()
}

fn methods(requests: &[Value]) -> Vec<&str> {
    requests
        .iter()
        .map(|request| request["method"].as_str().unwrap_or_default())
        .collect()
}

/// Full fake-Claude runtime path: matrix parent -> agent tool -> binder/executor
/// -> `claude-agent-acp` spawn -> scripted ACP agent (`rho __acp-fixture-agent`)
/// -> mode, session options, prompt -> artifacts -> parent completion ->
/// `/attach` replay. Never touches a real Claude install or the network.
#[test]
fn fake_claude_runtime_end_to_end_success() {
    let home = IsolatedHome::new().unwrap();
    let (fake, path) = install_fake(&home, "success.json");
    let mut harness = launch(&home, path, 32, "fake_claude_runtime_e2e");

    harness.submit_text("fixture claude agent").unwrap();
    // Wait for the parent's durable response to the completion notification,
    // not the short-lived launch receipt or a card that can scroll out of view.
    harness
        .wait_for_text(
            "claude-background-delivery-1: delegated result received",
            WaitTimeout::secs(30, "parent received delegated result"),
        )
        .unwrap();

    let run_dir = acp_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let run_id = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .expect("run id")
        .to_string();
    let status = acp_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    assert_eq!(
        json!({
            "state": status["state"],
            "agent_id": status["agent_id"],
            "provider": status["provider"],
            "model": status["model"],
            "result": status["result"],
            "runtime_session_id": status["runtime_session_id"],
            "runtime_model": status["runtime_model"],
            "cost_micros": status["total_cost_usd"]
                .as_f64()
                .map(|cost| (cost * 1_000_000.0).round() as u64),
        }),
        json!({
            "state": "ok",
            "agent_id": "claude-planner",
            "provider": "claude-code",
            "model": "claude-opus-demo",
            "result": "rho-claude-e2e-ok",
            "runtime_session_id": "scripted-session",
            "runtime_model": "claude-sonnet-5",
            "cost_micros": 34_271,
        })
    );
    // `/limits` keeps the rate-limit window the run observed.
    let limits =
        acp_e2e::read_json(&home.home.join(".rho/cache/claude-code/rate-limits.json")).unwrap();
    assert_eq!(
        (
            &limits["windows"][0]["info"]["rateLimitType"],
            &limits["windows"][0]["info"]["utilization"],
        ),
        (&json!("five_hour"), &json!(0.12)),
        "rate-limit cache: {limits}"
    );
    assert!(
        run_dir.starts_with(home.home.join(".rho/sessions")),
        "interactive run was not nested under its session: {}",
        run_dir.display()
    );
    assert!(
        !home.home.join(".rho/subagents").join(&run_id).exists(),
        "interactive run also appeared in the global pool"
    );

    // Spawn: the adapter takes no arguments and runs in the workspace.
    let spawn = fake.spawn_record();
    assert_eq!(
        (spawn.args, spawn.cwd.canonicalize().unwrap()),
        (Vec::<String>::new(), home.workspace.canonicalize().unwrap())
    );

    // Wire: no authenticate; the session carries identity and the fence in
    // `_meta`, Bypass sets `bypassPermissions`, and the task is the prompt.
    let requests = requests(&fake);
    assert_eq!(
        methods(&requests),
        [
            "initialize",
            "session/new",
            "session/set_mode",
            "session/prompt"
        ]
    );
    let meta = &requests[1]["params"]["_meta"];
    assert!(
        meta["systemPrompt"]
            .as_str()
            .is_some_and(|prompt| prompt.starts_with("Produce a short plan.")),
        "replace prompt must be the agent body: {meta}"
    );
    assert_eq!(
        (
            &meta["claudeCode"]["options"],
            &requests[2]["params"]["modeId"],
            &requests[3]["params"]["prompt"],
        ),
        (
            &json!({
                "tools": ["Read", "Edit", "Bash"],
                "allowedTools": ["Read", "Edit", "Bash(git *)"],
                "disallowedTools": ["Task", "Agent"],
                "settingSources": ["project"],
                "strictMcpConfig": true,
                "maxTurns": 10000,
                "promptSuggestions": false,
                "model": "claude-opus-demo",
            }),
            &json!("bypassPermissions"),
            &json!([{"type": "text", "text": "Say hello in one short sentence."}]),
        )
    );

    // Replay the real on-disk artifacts in the in-place attach view.
    attach_view::open(&mut harness, WaitTimeout::secs(10, "attach view"));
    for (text, label) in [
        (format!("attach {run_id}"), "attach startup"),
        ("Say hello in one short sentence.".into(), "attach prompt"),
        ("ho-claude-e2e-ok".into(), "attach final text tail"),
        ("ran as claude-sonnet-5".into(), "attach resolved model"),
        ("claude-planner".into(), "attach agent id"),
    ] {
        harness
            .wait_for_text(&text, WaitTimeout::secs(10, label))
            .unwrap();
    }
    attach_view::leave(&mut harness, WaitTimeout::secs(5, "leave attach view"));
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Background Claude completion path: the reported session cost must fold
/// into the parent session total shown by `/info` after automatic delivery.
#[test]
fn fake_claude_background_cost_appears_in_info() {
    background_cost_appears_in_info(Host::InProcess, "fake_claude_background_cost_info");
}

/// The same with the daemon running the session: the TUI reads the run's
/// cost from its artifacts rather than from an in-process manager.
#[test]
fn fake_claude_background_cost_appears_in_info_daemon() {
    background_cost_appears_in_info(Host::Daemon, "fake_claude_background_cost_info_daemon");
}

fn background_cost_appears_in_info(host: Host, name: &str) {
    let home = IsolatedHome::new().unwrap();
    let (_fake, path) = install_fake(&home, "success.json");
    let mut harness = launch_on(host, &home, path, 36, name);

    harness
        .submit_text("fixture background claude agent")
        .unwrap();
    let run_dir = acp_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(20));
    let status = acp_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    assert_eq!(status["state"], "ok", "status={status}");
    harness
        .wait_for_text(
            "claude-background-delivery-1:",
            WaitTimeout::secs(20, "completion delivery"),
        )
        .unwrap();
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
        !screen.contains("No token usage recorded yet."),
        "subagent cost should replace the empty-usage note:\n{screen}"
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Sibling error path: the fake ends the turn with `max_turn_requests`; the
/// parent surfaces a failed delegated run without network access.
#[test]
fn fake_claude_runtime_end_to_end_error() {
    let home = IsolatedHome::new().unwrap();
    let (fake, path) = install_fake(&home, "error.json");
    let mut harness = launch(&home, path, 32, "fake_claude_runtime_e2e_error");

    harness.submit_text("fixture claude agent error").unwrap();
    harness
        .wait_for_text(
            "claude-background-delivery-1: failed result received",
            WaitTimeout::secs(30, "parent received delegated failure"),
        )
        .unwrap();

    let run_dir = acp_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let status = acp_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    assert_eq!(
        json!({
            "state": status["state"],
            "agent_id": status["agent_id"],
            "error": status["error"],
        }),
        json!({
            "state": "error",
            "agent_id": "claude-planner",
            "error": "acp: turn ended with max_turn_requests",
        })
    );
    assert_eq!(fake.spawn_record().args, Vec::<String>::new());

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Full Claude Code advisor path in a daemon session: `/advisor on` picker
/// selection -> the daemon's settings -> advisor tool call ->
/// `claude-agent-acp` one-shot with no tools -> advice in the card. Never
/// touches a real Claude install or the network.
#[test]
fn fake_claude_advisor_reviews_the_session() {
    let home = IsolatedHome::new().unwrap();
    fs::write(
        &home.config_path,
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[behavior]
credential_store = "file"
"#,
    )
    .unwrap();
    let (fake, path) = install_fake(&home, "success.json");
    let plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path)
    .with_env("RHO_TUI_DAEMON", "1")
    .with_env("OPENAI_API_KEY", "sk-test-matrix");
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_advisor").unwrap();
    harness
        .wait_for_text("Bypass ·", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Picking a Claude Code row is the only step: it selects the runtime too.
    harness.submit_text("/advisor on").unwrap();
    harness
        .wait_for_text(
            "Advisor model · Type to search",
            WaitTimeout::secs(10, "advisor model picker"),
        )
        .unwrap();
    harness.type_text("claude-code/opus").unwrap();
    harness
        .wait_for_text("(1/1)", WaitTimeout::secs(10, "claude code row filtered"))
        .unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "advisor: on; applies next turn",
            WaitTimeout::secs(10, "advisor turned on"),
        )
        .unwrap();

    harness.submit_text("fixture advisor").unwrap();
    // The fixture's answer is the advice the executor gets back.
    harness
        .wait_for_text(
            "rho-claude-e2e-ok",
            WaitTimeout::secs(30, "advice in the advisor card"),
        )
        .unwrap();

    // Parity with the Rho advisor: one turn, no tools, no settings, no
    // resumable session, and Rho rejects anything Claude asks to run.
    let requests = requests(&fake);
    assert_eq!(
        methods(&requests),
        [
            "initialize",
            "session/new",
            "session/set_mode",
            "session/prompt"
        ]
    );
    let meta = &requests[1]["params"]["_meta"];
    let mut options = meta["claudeCode"]["options"].clone();
    // Effort follows the advisor's reasoning setting, covered by unit tests.
    options.as_object_mut().unwrap().remove("effort");
    assert_eq!(
        (options, &requests[2]["params"]["modeId"]),
        (
            json!({
                "tools": [],
                "disallowedTools": ["Task", "Agent"],
                "settingSources": [],
                "strictMcpConfig": true,
                "maxTurns": 1,
                "promptSuggestions": false,
                "model": "opus",
                "persistSession": false,
            }),
            &json!("default"),
        )
    );
    assert!(
        meta["systemPrompt"]
            .as_str()
            .is_some_and(|prompt| !prompt.is_empty()),
        "the advisor replaces Claude's system prompt: {meta}"
    );
    assert!(
        requests[3]["params"]["prompt"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("fixture advisor")),
        "the advisor must receive the session transcript: {}",
        requests[3]
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}
