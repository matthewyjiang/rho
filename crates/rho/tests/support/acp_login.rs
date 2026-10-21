//! Exercise the advertised terminal command exactly as an ACP host launches it.

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use rho_providers::provider::OPENAI_API_KEY_ACCOUNT;
use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use serde_json::json;

use super::{AcpServer, Workspace, REPLY_TIMEOUT};

// Covers: a cold-cache provider whose model discovery requires credentials
// must still advertise login, its session startup must answer auth_required
// so hosts show sign-in, and that failure must not kill ACP or block history
// deletion, which needs no model or credentials.
// Owner: ACP process startup; no model/credential fixtures mask discovery here.
#[test]
fn unauthenticated_cold_start_can_discover_login() {
    let workspace = Workspace::new();
    let saved_ids = {
        let mut fixture = workspace.server();
        fixture
            .request("initialize", json!({"protocolVersion": 1}))
            .0
            .unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let (session, _) = fixture.request(
                "session/new",
                json!({"cwd": workspace.project, "mcpServers": []}),
            );
            let id = session.unwrap()["sessionId"].as_str().unwrap().to_owned();
            fixture.prompt(&id, "save history for deletion");
            fixture
                .request("session/close", json!({"sessionId": id}))
                .0
                .unwrap();
            ids.push(id);
        }
        ids
    };
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .env_clear()
        .env("HOME", workspace.home.path())
        .env("RHO_HOME", workspace.home.path().join(".rho"))
        .env("RHO_CREDENTIAL_STORE", "file")
        .current_dir(&workspace.project)
        .arg("--config")
        .arg(workspace.home.path().join("config.toml"))
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
    let (deleted, _) = server.request("session/delete", json!({"sessionId": saved_ids[0]}));
    assert_eq!(deleted.unwrap(), json!({}));
    let (session, _) = server.request(
        "session/new",
        json!({"cwd": workspace.project, "mcpServers": []}),
    );
    let failure = session.expect_err("credentials are still required for a session");
    assert_eq!(
        (failure["code"].clone(), failure.get("data")),
        (json!(-32000), None),
        "missing credentials ask the host to sign in: {failure}"
    );
    let (after_failure, _) = server.request("initialize", initialize);
    assert_eq!(after_failure.unwrap(), initialized);
    let (deleted, _) = server.request("session/delete", json!({"sessionId": saved_ids[1]}));
    assert_eq!(deleted.unwrap(), json!({}));
    let (listed, _) = server.request("session/list", json!({}));
    assert_eq!(listed.unwrap()["sessions"], json!([]));
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

// Covers: a host that draws URL elicitations signs in to OAuth without a
// terminal: `authenticate` shows the login URL through a URL elicitation tied
// to that request, and a declined elicitation, a cancelled `authenticate`, or a
// `logout` (which must not let the login save afterwards) fails it as
// cancelled and releases the login's callback listener, so the next attempt is
// not left fighting a stale one.
// Owner: ACP authenticate over real stdio. OpenRouter's browser login is
// local until its callback, so no provider is contacted.
#[test]
fn authenticate_shows_the_login_url_and_abandons_it_when_the_host_does() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    let (initialized, _) = server.request(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {"auth": {"terminal": true}, "elicitation": {"url": {}}},
        }),
    );
    let methods = initialized.unwrap()["authMethods"].clone();
    let method = |id: &str| {
        methods
            .as_array()
            .unwrap()
            .iter()
            .find(|method| method["id"] == id)
            .unwrap_or_else(|| panic!("{id} is advertised"))
            .clone()
    };
    // An untagged method is ACP's default, agent-run sign-in.
    let (oauth, api_key) = (method("openrouter-oauth"), method("openrouter-api-key"));
    assert_eq!(
        (oauth.get("type"), oauth.get("args"), &api_key["type"]),
        (None, None, &json!("terminal"))
    );

    /// How the host leaves the sign-in.
    #[derive(Clone, Copy, Debug)]
    enum Abandon {
        Decline,
        Cancel,
        Logout,
    }
    for abandon in [Abandon::Decline, Abandon::Cancel, Abandon::Logout] {
        server.next_id += 1;
        let id = server.next_id;
        server.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "authenticate",
            "params": {"methodId": "openrouter-oauth"},
        }));
        let shown = server
            .messages
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the login URL is shown");
        assert_eq!(
            (
                shown["method"].clone(),
                shown["params"]["mode"].clone(),
                shown["params"]["requestId"].clone()
            ),
            (json!("elicitation/create"), json!("url"), json!(id)),
            "{shown}"
        );
        let url = url::Url::parse(shown["params"]["url"].as_str().unwrap()).unwrap();
        assert_eq!(url.origin().ascii_serialization(), "https://openrouter.ai");
        let callback = url
            .query_pairs()
            .find(|(key, _)| key == "callback_url")
            .map(|(_, value)| url::Url::parse(&value).unwrap())
            .expect("the login waits on a local callback");
        let listener = format!(
            "{}:{}",
            callback.host_str().unwrap(),
            callback.port().unwrap()
        );
        assert!(
            std::net::TcpStream::connect(&listener).is_ok(),
            "listening while shown"
        );

        let logout_id = id + 1;
        let mut logged_out = true;
        match abandon {
            Abandon::Decline => server.send(&json!({
                "jsonrpc": "2.0", "id": shown["id"], "result": {"action": "decline"},
            })),
            Abandon::Cancel => server.send(&json!({
                "jsonrpc": "2.0", "method": "$/cancel_request", "params": {"requestId": id},
            })),
            Abandon::Logout => {
                server.next_id = logout_id;
                server.send(&json!({
                    "jsonrpc": "2.0", "id": logout_id, "method": "logout", "params": {},
                }));
                logged_out = false;
            }
        }
        let mut reply = None;
        while reply.is_none() || !logged_out {
            let message = server
                .messages
                .recv_timeout(REPLY_TIMEOUT)
                .expect("authenticate answers");
            if message.get("method").is_some() {
                // A cancelled authenticate withdraws its pending elicitation.
                assert_eq!(message["method"], "$/cancel_request", "{message}");
            } else if message["id"] == id {
                reply = Some(message);
            } else {
                assert_eq!(
                    (&message["id"], &message["result"]),
                    (&json!(logout_id), &json!({})),
                    "{message}"
                );
                logged_out = true;
            }
        }
        let reply = reply.unwrap();
        assert_eq!(
            reply["error"]["code"],
            json!(-32800),
            "{abandon:?}: {reply}"
        );
        assert!(
            std::net::TcpStream::connect(&listener).is_err(),
            "{abandon:?} releases the callback listener"
        );
    }
}
