//! Sign-in recovery against a provider that checks its key over real HTTP.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

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
    project: PathBuf,
    store: FileCredentialStore,
}

impl MockProviderHome {
    fn new(valid: &Arc<Mutex<String>>) -> Self {
        let address = spawn_provider(Arc::clone(valid));
        let home = IsolatedHome::new().unwrap();
        let project = home.home.join("project");
        std::fs::create_dir_all(&project).unwrap();
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
        Self {
            home,
            project,
            store,
        }
    }

    /// Starts and initializes `rho acp`, returning the `initialize` result.
    fn server(&self) -> (AcpServer, Value) {
        // An explicit provider makes startup discover its models, so the
        // missing key fails startup itself rather than the session build.
        self.server_with_args(&["--provider", "mock", "acp"])
    }

    fn server_with_args(&self, args: &[&str]) -> (AcpServer, Value) {
        let mut command = self.home.command(env!("CARGO_BIN_EXE_rho"));
        command
            .current_dir(&self.project)
            .arg("--config")
            .arg(&self.home.config_path)
            .args(args);
        let mut server = AcpServer::from_command(&mut command);
        let initialized = server
            .request("initialize", json!({"protocolVersion": 1}))
            .0
            .unwrap();
        (server, initialized)
    }

    fn new_session(&self) -> Value {
        json!({"cwd": self.project, "mcpServers": []})
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

// Covers: a missing key and a key the provider revokes mid-session must both
// answer ACP auth_required (not internal_error), for turns and compaction,
// without echoing the key or the provider's diagnostic, and each sign-in must
// apply without reconnecting: startup runs again, and the next prompt reloads
// the refused credentials for turns and compaction alike, because providers
// capture them when built.
// Owner: ACP request error mapping and session provider lifecycle, over real
// stdio and HTTP; no fixture provider stands in for the credential path.
#[test]
fn credential_failures_require_auth_and_each_sign_in_applies_in_place() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = MockProviderHome::new(&valid);
    let (mut server, _) = fixture.server();
    assert_auth_required(
        server.request("session/new", fixture.new_session()).0,
        "no key is saved yet",
    );

    // A sign-in saves the key; the same connection starts without reconnecting.
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

// Covers: `logout` must be advertised and delete the selected auth profile's
// stored key, after which new sessions and a live session's next turn fail
// auth_required even though the provider still accepts the key that session
// holds, and a sign-in afterwards applies in place again.
// Owner: ACP logout and session provider lifecycle, over real stdio and HTTP.
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

// Covers: ACP `logout` on a keyless Ollama login signs it out: the
// `[providers.ollama]` login record goes, not only stored credentials, and
// later sessions in the same process need a sign-in.
// Owner: ACP logout of an endpoint-recorded login, over real stdio.
#[test]
fn logout_forgets_a_keyless_ollama_login() {
    let fixture = MockProviderHome::new(&Arc::new(Mutex::new(FIRST_KEY.to_owned())));
    let config_path = fixture.home.config_path.clone();
    // The fixture host refuses keyless requests, so no Ollama models are found.
    let config = std::fs::read_to_string(&config_path)
        .unwrap()
        .replace(
            "provider = \"mock\"\nmodel = \"mock-model\"\nauth = \"mock-api-key\"",
            "provider = \"ollama\"\nmodel = \"llama\"\nauth = \"none\"",
        )
        .replace("[providers.custom.mock]", "[providers.ollama]");
    std::fs::write(&config_path, config).unwrap();
    let (mut server, _) = fixture.server_with_args(&["acp"]);
    server
        .request("session/new", fixture.new_session())
        .0
        .expect("a logged-in keyless Ollama opens sessions");

    let (logged_out, _) = server.request("logout", json!({}));
    assert_eq!(logged_out.unwrap(), json!({}));
    let config = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !config.contains("[providers.ollama]"),
        "logout must remove the Ollama login record:\n{config}"
    );
    // The process loaded config before the logout; sessions must still see it.
    assert_auth_required(
        server.request("session/new", fixture.new_session()).0,
        "new sessions need a sign-in after logout",
    );
}

// Covers: an Ollama login recorded after `rho acp` started (another process,
// or ACP sign-in) is honored by the next session, built against the API base
// that login chose rather than the built-in default.
// Owner: ACP session build from the on-disk Ollama login record.
#[test]
fn a_later_ollama_login_builds_against_its_recorded_endpoint() {
    let fixture = MockProviderHome::new(&Arc::new(Mutex::new(FIRST_KEY.to_owned())));
    let config_path = fixture.home.config_path.clone();
    let mock_config = std::fs::read_to_string(&config_path).unwrap();
    let base_url = mock_config
        .lines()
        .find_map(|line| line.strip_prefix("base_url = "))
        .unwrap()
        .to_owned();
    let without_login = mock_config
        .replace(
            "provider = \"mock\"\nmodel = \"mock-model\"\nauth = \"mock-api-key\"",
            "provider = \"ollama\"\nmodel = \"mock-model\"\nauth = \"ollama-api-key\"",
        )
        .replace(
            &format!("[providers.custom.mock]\nbase_url = {base_url}\n"),
            "",
        );
    std::fs::write(&config_path, &without_login).unwrap();
    fixture
        .store
        .set_secret(rho_providers::provider::OLLAMA_API_KEY_ACCOUNT, FIRST_KEY)
        .unwrap();
    let (mut server, _) = fixture.server_with_args(&["acp"]);
    assert_auth_required(
        server.request("session/new", fixture.new_session()).0,
        "a key alone is not an Ollama login",
    );

    std::fs::write(
        &config_path,
        format!("{without_login}\n[providers.ollama]\nbase_url = {base_url}\n"),
    )
    .unwrap();
    let (session, _) = server.request("session/new", fixture.new_session());
    let session_id = session.expect("the recorded login opens sessions")["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(server.prompt(&session_id, "hi"), "answered");
}
