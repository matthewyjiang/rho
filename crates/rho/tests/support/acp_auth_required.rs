//! Sign-in recovery against a provider that checks its key over real HTTP.

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use rho_tui_pty::IsolatedHome;
use serde_json::{json, Value};

use super::{key_checking_provider::spawn_provider, AcpServer};

const FIRST_KEY: &str = "test-first-key";
const SECOND_KEY: &str = "test-second-key";
const KEY_ACCOUNT: &str = "provider:mock:api-key";

/// A home whose config selects the custom `mock` provider at an
/// [`spawn_provider`] host, with an empty file credential store.
struct MockProviderHome {
    home: IsolatedHome,
    store: FileCredentialStore,
}

impl MockProviderHome {
    fn new(valid: &Arc<Mutex<String>>) -> Self {
        let address = spawn_provider(Arc::clone(valid));
        let home = IsolatedHome::new().unwrap();
        std::fs::write(
            &home.config_path,
            format!(
                r#"provider = "mock"
model = "mock-model"
auth = "mock-api-key"

[providers.custom.mock]
base_url = "http://{address}/v1"

[behavior]
permission_mode = "plan"
check_for_updates = false

[web_search]
hosted = false
provider = "disabled"
"#
            ),
        )
        .unwrap();
        let store = FileCredentialStore::with_rho_home(home.rho_home()).unwrap();
        Self { home, store }
    }

    /// Starts and initializes `rho acp`, returning the `initialize` result.
    fn server(&self) -> (AcpServer, Value) {
        let mut command = self.home.command(env!("CARGO_BIN_EXE_rho"));
        command.arg("acp");
        let mut server = AcpServer::from_command(&mut command);
        let initialized = server
            .request("initialize", json!({"protocolVersion": 1}))
            .0
            .unwrap();
        (server, initialized)
    }

    fn new_session(&self) -> Value {
        json!({"cwd": self.home.workspace, "mcpServers": []})
    }
}

/// Asserts `reply` is ACP `auth_required` with no data and no key in it.
fn assert_auth_required(reply: Result<Value, Value>, what: &str) {
    let error = reply.expect_err(what);
    assert_eq!(
        (error["code"].clone(), error.get("data")),
        (json!(-32000), None),
        "{what}: {error}"
    );
    assert!(!error.to_string().contains(FIRST_KEY), "{error}");
}

fn prompt(session_id: &str, text: &str) -> Value {
    json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]})
}

// Covers: a missing key, at session/new, and a key the provider revokes
// mid-session must both answer ACP auth_required (not invalid_params or
// internal_error), for turns and compaction, without echoing the key or the
// provider's diagnostic, and each sign-in must apply without reconnecting:
// the daemon opens the next session, and the daemon session's next prompt
// reloads the refused credentials for turns and compaction alike, because
// providers capture them when built.
// Owner: ACP prompt error mapping over the daemon's turn outcome, and the
// daemon session's provider lifecycle, over real stdio and HTTP; no fixture
// provider stands in for the credential path.
#[test]
fn credential_failures_require_auth_and_each_sign_in_applies_in_place() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = MockProviderHome::new(&valid);
    let (mut server, _) = fixture.server();
    assert_auth_required(
        server.request("session/new", fixture.new_session()).0,
        "no key is saved yet",
    );

    // A sign-in saves the key; the same connection opens without reconnecting.
    fixture.store.set_secret(KEY_ACCOUNT, FIRST_KEY).unwrap();
    let (session, _) = server.request("session/new", fixture.new_session());
    let session_id = session.unwrap()["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(server.prompt(&session_id, "hi"), "answered");

    *valid.lock().unwrap() = SECOND_KEY.to_owned();
    assert_auth_required(
        server
            .request("session/prompt", prompt(&session_id, "hi again"))
            .0,
        "the provider revoked the key",
    );
    assert_auth_required(
        server
            .request("session/prompt", prompt(&session_id, "/compact"))
            .0,
        "compaction is refused too",
    );

    fixture.store.set_secret(KEY_ACCOUNT, SECOND_KEY).unwrap();
    assert_eq!(server.prompt(&session_id, "signed in"), "answered");
    // Compaction summarizes with its own handle on the provider.
    server
        .request("session/prompt", prompt(&session_id, "/compact"))
        .0
        .expect("compaction uses the renewed key");
}

// Covers: `logout` must be advertised and delete the stored key of the auth
// profile the config selects, after which a live session's next turn fails
// auth_required even though the provider still accepts the key that session
// holds, and a sign-in afterwards applies in place again.
// Owner: ACP logout through the daemon and the daemon session's provider
// lifecycle, over real stdio and HTTP.
#[test]
fn logout_requires_a_new_sign_in_for_new_and_live_sessions() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = MockProviderHome::new(&valid);
    fixture.store.set_secret(KEY_ACCOUNT, FIRST_KEY).unwrap();
    let (mut server, initialized) = fixture.server();
    assert_eq!(
        initialized["agentCapabilities"]["auth"],
        json!({"logout": {}})
    );
    let (session, _) = server.request("session/new", fixture.new_session());
    let session_id = session.unwrap()["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(server.prompt(&session_id, "hi"), "answered");

    let (logged_out, _) = server.request("logout", json!({}));
    assert_eq!(logged_out.unwrap(), json!({}));
    assert_eq!(fixture.store.get_secret(KEY_ACCOUNT).unwrap(), None);
    assert_auth_required(
        server
            .request("session/prompt", prompt(&session_id, "still there?"))
            .0,
        "the live session no longer holds a sign-in",
    );
    assert_auth_required(
        server.request("session/new", fixture.new_session()).0,
        "new sessions need a sign-in",
    );

    fixture.store.set_secret(KEY_ACCOUNT, FIRST_KEY).unwrap();
    assert_eq!(server.prompt(&session_id, "signed in again"), "answered");
}
