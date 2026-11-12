//! Fake Antigravity ACP runtime delegation, agent allowlist, mode fence, and
//! permission answers.

use pretty_assertions::assert_eq;
use rho_tui_pty::{IsolatedHome, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

use super::acp_e2e::{self, FakeAcpAgent};

const FAKE_ANTIGRAVITY: FakeAcpAgent = FakeAcpAgent {
    program: "agy",
    fixtures: "antigravity_acp",
    recorded_env: None,
    acp_subcommand: Some("acp"),
    prelude: "",
};

/// Full fake-Antigravity ACP path: matrix parent -> agent tool ->
/// binder/executor -> `agy acp` spawn -> scripted ACP agent
/// (`rho __acp-fixture-agent`) -> mode, model option, permission answers ->
/// artifacts -> parent completion.
/// Offline; no real Antigravity server. Attach replay is shared ACP rendering
/// covered by the Cursor scenario.
#[test]
fn fake_antigravity_acp_runtime_end_to_end() {
    let home = IsolatedHome::new().unwrap();
    FAKE_ANTIGRAVITY.install_agent(&home.home, "antigravity-worker.md");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let fake = FAKE_ANTIGRAVITY.install(&home.path().join("fake-antigravity"), &binary);
    let path = acp_e2e::path_with_fake(&fake.bin_dir);
    assert_eq!(
        acp_e2e::which_on_path("agy", &path).as_deref(),
        Some(fake.program.as_path()),
        "PATH must resolve the fake agy first"
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

    let run_dir = acp_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let status = acp_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
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
            "model": "gemini-3.8-flash",
            "runtime": "antigravity",
            "result": "rho-antigravity-acp-e2e-ok",
        })
    );

    // Spawn: `agy acp` with Rho's own agy settings directory and the run's
    // agy agent, whose `tools:` is the declared allowlist, in the workspace.
    let spawn = fake.spawn_record();
    let agent_root = run_dir.join("agy");
    let name = spawn.args.last().cloned().unwrap_or_default();
    // A daemon names the run by its home's real path, which on macOS is not
    // the temp dir's spelling.
    let added_dir = spawn.args.get(4).cloned().unwrap_or_default();
    assert_eq!(
        spawn.args,
        [
            "acp",
            "--cli_dir_name",
            "rho",
            "--add-dir",
            &added_dir,
            "--agent",
            &name,
        ]
    );
    assert_eq!(
        std::path::Path::new(&added_dir).canonicalize().unwrap(),
        agent_root.canonicalize().unwrap()
    );
    let agent =
        std::fs::read_to_string(agent_root.join(format!(".agents/agents/{name}.md"))).unwrap();
    assert!(
        agent.contains(&format!("name: {name}\n"))
            && agent.contains("tools:\n  - read_file\n  - run_command\n"),
        "{agent}"
    );
    assert_eq!(
        spawn.cwd.canonicalize().unwrap(),
        home.workspace.canonicalize().unwrap()
    );

    // Wire: no authenticate; Bypass sets mode `default` and the pinned model
    // before the prompt, and approves the permission request once.
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
            &requests[2]["params"]["modeId"],
            &requests[3]["params"]["configId"],
            &requests[3]["params"]["value"],
        ),
        (
            &json!("default"),
            &json!("model"),
            &json!("gemini-3.8-flash"),
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
    let expected = [selected("allow")];
    assert_eq!(replies, expected.iter().collect::<Vec<_>>());
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}
