//! Exercise the advertised terminal command exactly as an ACP host launches it.

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use rho_providers::provider::OPENAI_API_KEY_ACCOUNT;
use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use serde_json::json;

use super::{AcpServer, Workspace, REPLY_TIMEOUT};

// Covers: a cold-cache provider whose model discovery requires credentials
// must still advertise login, and a failed session startup must not kill ACP.
// Owner: ACP process startup; no model/credential fixtures mask discovery here.
#[test]
fn unauthenticated_cold_start_can_discover_login() {
    let home = IsolatedHome::new().unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .env_clear()
        .env("HOME", &home.home)
        .env("RHO_HOME", home.home.join(".rho"))
        .env("RHO_CREDENTIAL_STORE", "file")
        .current_dir(&home.workspace)
        .arg("--config")
        .arg(&home.config_path)
        .args(["--provider", "github-copilot", "acp", "--t3code"]);
    let mut server = AcpServer::from_command(&mut command);
    let initialize =
        json!({"protocolVersion": 1, "clientCapabilities": {"auth": {"terminal": true}}});
    let (initialized, _) = server.request("initialize", initialize.clone());
    let initialized = initialized.unwrap();
    let copilot = initialized["authMethods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|method| method["id"] == "github-copilot")
        .expect("login is discoverable without Copilot credentials or models");
    assert_eq!(
        (copilot["type"].clone(), copilot["args"].clone()),
        (json!("terminal"), json!(["--login", "github-copilot"]))
    );
    let (session, _) = server.request(
        "session/new",
        json!({"cwd": home.workspace, "mcpServers": []}),
    );
    assert_eq!(
        session.expect_err("credentials are still required for a session")["code"],
        json!(-32603)
    );
    let (after_failure, _) = server.request("initialize", initialize);
    assert_eq!(after_failure.unwrap(), initialized);
}

// Covers: terminal methods must append to the configured
// ACP invocation (including --t3code), and save secrets without terminal echo.
// Cancellation/invalid input must not return success or replace credentials.
// Owner: ACP host login boundary, real stdio handshake and terminal subprocess.
#[test]
fn advertised_terminal_login_saves_or_cancels_without_echoing_keys() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    let (initialized, _) = server.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {"auth": {"terminal": true}}}),
    );
    let initialized = initialized.unwrap();
    let method = initialized["authMethods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|method| method["id"] == "api-key")
        .expect("OpenAI API-key login is advertised");
    assert_eq!(method["type"], json!("terminal"));
    let args: Vec<String> = serde_json::from_value(method["args"].clone()).unwrap();
    drop(server);

    for (name, paste, key, expected_code, expected_secret) in [
        ("save", "test-acp-secret", Key::Enter, 0, "test-acp-secret"),
        ("cancel", "test-replacement", Key::Esc, 1, "original-key"),
        (
            "interrupt",
            "test-replacement",
            Key::Ctrl('c'),
            1,
            "original-key",
        ),
        ("blank", "", Key::Enter, 1, "original-key"),
        (
            "multiline",
            "test-first\ntest-second",
            Key::Enter,
            1,
            "original-key",
        ),
    ] {
        let home = IsolatedHome::new().unwrap();
        let store = FileCredentialStore::with_rho_home(home.home.join(".rho")).unwrap();
        store
            .set_secret(OPENAI_API_KEY_ACCOUNT, "original-key")
            .unwrap();
        let config_before = std::fs::read(&home.config_path).unwrap();
        let mut plan = RhoLaunchPlan::matrix(
            env!("CARGO_BIN_EXE_rho"),
            &home,
            PtySize {
                rows: 24,
                cols: 100,
            },
        )
        .with_arg("acp")
        .with_arg("--t3code");
        plan.args.extend(args.clone());
        let mut terminal = PtyHarness::spawn_named(&plan, format!("acp_login_{name}")).unwrap();
        terminal
            .wait_for_text(
                "input hidden",
                WaitTimeout::new(REPLY_TIMEOUT, "terminal login prompt"),
            )
            .unwrap();
        let mut input = rho_tui_pty::encode_paste(paste);
        input.extend(rho_tui_pty::encode_key(&key));
        terminal.inject_bytes(&input).unwrap();
        assert_eq!(
            terminal
                .wait_for_exit(WaitTimeout::new(REPLY_TIMEOUT, "terminal login exit"))
                .unwrap(),
            expected_code,
            "{name}"
        );
        assert_eq!(
            store.get_secret(OPENAI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some(expected_secret),
            "{name}"
        );
        assert_eq!(
            std::fs::read(&home.config_path).unwrap(),
            config_before,
            "login does not switch the configured model"
        );
        if !paste.is_empty() {
            assert!(
                !String::from_utf8_lossy(terminal.raw_output())
                    .contains(paste.lines().next().unwrap()),
                "secret leaked into the terminal for {name}"
            );
        }
        assert!(
            terminal.raw_sequence_occurrences(b"\x1b[?2004l") > 0,
            "bracketed paste restored for {name}"
        );
    }
}
