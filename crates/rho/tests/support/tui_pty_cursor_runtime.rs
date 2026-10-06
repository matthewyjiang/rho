//! Fake Cursor ACP runtime delegation, fence, permission answers, and attach.

use pretty_assertions::assert_eq;
use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Duration};

use super::{claude_e2e, cursor_acp_e2e};

/// Full fake-Cursor ACP path: matrix parent -> agent tool -> binder/executor
/// -> `cursor-agent acp` spawn with a managed `CURSOR_CONFIG_DIR` -> scripted
/// ACP agent (`rho __acp-fixture-agent`) -> permission answers -> artifacts ->
/// parent completion -> `rho attach` replay. Offline; no real Cursor binary.
#[test]
fn fake_cursor_acp_runtime_end_to_end() {
    let home = IsolatedHome::new().unwrap();
    cursor_acp_e2e::install_cursor_worker_agent(&home.home);
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let fake = cursor_acp_e2e::install_fake_cursor_agent(&home.path().join("fake-cursor"), &binary);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);
    assert_eq!(
        claude_e2e::which_on_path("cursor-agent", &path).as_deref(),
        Some(fake.cursor_agent.as_path()),
        "PATH must resolve the fake cursor-agent first"
    );

    let plan = RhoLaunchPlan::matrix(
        binary.clone(),
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_cursor_acp_runtime_e2e").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("fixture cursor agent").unwrap();
    harness
        .wait_for_text(
            "cursor-background-delivery-1: delegated result received",
            WaitTimeout::secs(30, "parent received delegated result"),
        )
        .unwrap();

    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let run_id = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .expect("run id")
        .to_string();
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    let observed = json!({
        "state": status["state"],
        "agent_id": status["agent_id"],
        "provider": status["provider"],
        "runtime": status["runtime"],
        "result": status["result"],
        "session_id": status["claude_session_id"],
    });
    assert_eq!(
        observed,
        json!({
            "state": "ok",
            "agent_id": "cursor-worker",
            "provider": "cursor",
            "runtime": "cursor",
            "result": "rho-cursor-acp-e2e-ok",
            "session_id": "scripted-session",
        })
    );

    // Spawn: identity-only argv, workspace cwd, and the per-run managed config
    // whose deny list fences writes (read, grep, and shell are declared).
    let spawn = fake.spawn_record();
    assert_eq!(spawn.args, ["--trust", "--model", "composer-2.5", "acp"]);
    assert_eq!(
        spawn.cwd.canonicalize().unwrap(),
        home.workspace.canonicalize().unwrap()
    );
    assert_eq!(spawn.config_dir, run_dir.join("cursor-config"));
    assert_eq!(
        claude_e2e::read_json(&spawn.config_dir.join("cli-config.json")).unwrap(),
        json!({
            "approvalMode": "allowlist",
            "autoAcceptWebSearch": false,
            "permissions": {"allow": [], "deny": ["Write(**)"]},
        })
    );

    // Wire: Bypass never switches mode or authenticates; declared shell is
    // allowed once, and Cursor-shaped web search (kind `search`, like the
    // declared grep) is rejected because no network tool is declared.
    let journal = fake.journal();
    let methods: Vec<&str> = journal
        .iter()
        .filter_map(|entry| entry["request"]["method"].as_str())
        .collect();
    assert_eq!(methods, ["initialize", "session/new", "session/prompt"]);
    let prompt = journal
        .iter()
        .find(|entry| entry["request"]["method"] == "session/prompt")
        .and_then(|entry| entry["request"]["params"]["prompt"][0]["text"].as_str())
        .expect("prompt text");
    assert!(
        prompt.ends_with("Run the tests and report."),
        "system prompt must precede the delegated prompt: {prompt}"
    );
    let replies: Vec<&Value> = journal
        .iter()
        .filter_map(|entry| entry.get("reply"))
        .collect();
    let selected = |option: &str| {
        json!({
            "method": "session/request_permission",
            "result": {"outcome": {"outcome": "selected", "optionId": option}},
        })
    };
    let expected = [selected("allow-once"), selected("reject-once")];
    assert_eq!(replies, expected.iter().collect::<Vec<_>>());
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    // Replay the on-disk artifacts: the shell card and final text survive.
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
    let mut attach = PtyHarness::spawn_named(&plan, "fake_cursor_acp_runtime_e2e_attach").unwrap();
    attach
        .wait_for_text("cargo test", WaitTimeout::secs(10, "attach shell card"))
        .unwrap();
    attach
        .wait_for_text(
            "rho-cursor-acp-e2e-ok",
            WaitTimeout::secs(5, "attach final text"),
        )
        .unwrap();
    attach.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(
        attach
            .wait_for_exit(WaitTimeout::secs(5, "detach"))
            .unwrap(),
        0
    );
}
