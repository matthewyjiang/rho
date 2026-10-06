//! Fake Antigravity ACP runtime delegation, fence, and permission answers.

use pretty_assertions::assert_eq;
use rho_tui_pty::{IsolatedHome, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Duration};

use super::{acp_e2e::FakeAcpAgent, claude_e2e};

const FAKE_ANTIGRAVITY: FakeAcpAgent = FakeAcpAgent {
    program: "agy_acp_server.par",
    fixtures: "antigravity_acp",
    recorded_env: "ANTIGRAVITY_HARNESS_PATH",
    acp_subcommand: None,
};

/// Full fake-Antigravity ACP path: matrix parent -> agent tool ->
/// binder/executor -> sign-in preflight -> `agy_acp_server.par` spawn ->
/// scripted ACP agent (`rho __acp-fixture-agent`) -> mode, tool allowlist,
/// model option, permission answers -> artifacts -> parent completion.
/// Offline; no real Antigravity server. Attach replay is shared ACP rendering
/// covered by the Cursor scenario.
#[test]
fn fake_antigravity_acp_runtime_end_to_end() {
    let home = IsolatedHome::new().unwrap();
    FAKE_ANTIGRAVITY.install_agent(&home.home, "antigravity-worker.md");
    let acp_home = home.home.join(".gemini/antigravity-acp");
    fs::create_dir_all(&acp_home).unwrap();
    fs::write(
        acp_home.join("settings.json"),
        r#"{"auth":{"type":"oauth-personal"}}"#,
    )
    .unwrap();
    fs::write(acp_home.join("acp_token.json"), "{}").unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let fake = FAKE_ANTIGRAVITY.install(&home.path().join("fake-antigravity"), &binary);
    let harness_file = fake.bin_dir.join("localharness_external");
    fs::write(&harness_file, "").unwrap();
    let path = claude_e2e::path_with_fake(&fake.bin_dir);
    assert_eq!(
        claude_e2e::which_on_path("agy_acp_server.par", &path).as_deref(),
        Some(fake.program.as_path()),
        "PATH must resolve the fake server first"
    );

    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_antigravity_acp_runtime_e2e").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("fixture antigravity agent").unwrap();
    harness
        .wait_for_text(
            "antigravity-background-delivery-1: delegated result received",
            WaitTimeout::secs(30, "parent received delegated result"),
        )
        .unwrap();

    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    assert_eq!(
        json!({
            "state": status["state"],
            "agent_id": status["agent_id"],
            "provider": status["provider"],
            "model": status["model"],
            "runtime": status["runtime"],
            "result": status["result"],
        }),
        json!({
            "state": "ok",
            "agent_id": "antigravity-worker",
            "provider": "antigravity",
            "model": "gemini-3.8-flash-low",
            "runtime": "antigravity",
            "result": "rho-antigravity-acp-e2e-ok",
        })
    );

    // Spawn: Linux needs `--uid=`; workspace cwd; the harness pinned next to
    // the canonical server.
    let spawn = fake.spawn_record();
    let expected_args: &[&str] = if cfg!(target_os = "linux") {
        &["--uid="]
    } else {
        &[]
    };
    assert_eq!(spawn.args, expected_args);
    assert_eq!(
        spawn.cwd.canonicalize().unwrap(),
        home.workspace.canonicalize().unwrap()
    );
    assert_eq!(
        PathBuf::from(&spawn.env),
        harness_file.canonicalize().unwrap()
    );

    // Wire: no authenticate; mode `default` and the pinned model are set
    // before the prompt; `session/new` allowlists exactly the declared
    // built-ins; declared run_command is allowed, undeclared fetch denied.
    let journal = fake.journal();
    let requests: Vec<&Value> = journal
        .iter()
        .filter_map(|entry| entry.get("request"))
        .collect();
    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        [
            "initialize",
            "session/new",
            "session/set_mode",
            "session/set_config_option",
            "session/prompt"
        ]
    );
    assert_eq!(
        (
            &requests[1]["params"]["_meta"],
            &requests[2]["params"]["modeId"],
            &requests[3]["params"]["configId"],
            &requests[3]["params"]["value"],
        ),
        (
            &json!({"agy": {"enabledTools": ["view_file", "run_command"]}}),
            &json!("default"),
            &json!("model"),
            &json!("gemini-3.8-flash-low"),
        )
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
    let expected = [selected("allow"), selected("deny")];
    assert_eq!(replies, expected.iter().collect::<Vec<_>>());
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}
