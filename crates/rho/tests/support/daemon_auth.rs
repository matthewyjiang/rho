//! Signing the daemon in and out over its socket (#1662): a key one client
//! saves or deletes applies to another client's running session from its
//! next prompt, against a provider that checks every key over real HTTP.

use std::{
    process::Stdio,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{AuthKey, AuthLogout, JobList, SessionNew, SessionPrompt},
    AuthKeyParams, AuthKeyResponse, AuthLogoutParams, AuthLogoutResponse, Client, ClientError,
    ClientInfo, EnvironmentOverride, ErrorCode, JobListParams, JobState, RpcError, SessionId,
    SessionNewParams, SessionPromptParams, TurnError, TurnErrorKind, TurnOutcome,
};
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use serde_json::Value;
use tokio::net::UnixStream;

use super::{
    harness::{DaemonProcess, DaemonSandbox},
    key_checking_provider::spawn_provider,
    sessions::DEADLINE,
};

const FIRST_KEY: &str = "test-first-key";
const SECOND_KEY: &str = "test-second-key";
const WRONG_KEY: &str = "test-wrong-key";
const AUTH: &str = "mock-api-key";
const KEY_ACCOUNT: &str = "provider:mock:api-key";
/// The variable that overrides the custom `mock` provider's saved key.
const KEY_VARIABLE: &str = "RHO_MOCK_API_KEY";

/// A daemon whose config selects the custom `mock` provider at a
/// [`spawn_provider`] host, with a file credential store in its home.
struct Fixture {
    sandbox: DaemonSandbox,
    workspace: std::path::PathBuf,
    store: FileCredentialStore,
    _daemon: DaemonProcess,
}

impl Fixture {
    /// Starts the daemon with the variables `environment` set.
    async fn start(valid: &Arc<Mutex<String>>, environment: &[(&str, &str)]) -> Self {
        let address = spawn_provider(Arc::clone(valid));
        let sandbox = DaemonSandbox::new();
        std::fs::write(
            sandbox.home().join("config.toml"),
            format!(
                r#"provider = "mock"
model = "mock-model"
auth = "{AUTH}"

[providers.custom.mock]
base_url = "http://{address}/v1"

[behavior]
check_for_updates = false

[web_search]
hosted = false
provider = "disabled"
"#
            ),
        )
        .unwrap();
        let workspace = sandbox.root().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = FileCredentialStore::with_rho_home(sandbox.home()).unwrap();
        let mut command = sandbox.rho();
        command
            // Outside the session's workspace, as a service manager starts it.
            .current_dir(sandbox.home())
            // The test must not query the developer's OS keyring, and the
            // session must reach the real provider, not the fixture one.
            .env("RHO_CREDENTIAL_STORE", "file")
            .env("XDG_CACHE_HOME", sandbox.root().join(".cache"))
            .env_remove("RHO_AUTOMATION_TEST_MODE")
            .env_remove(KEY_VARIABLE)
            .envs(environment.iter().copied())
            .args(["daemon", "run"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let daemon = DaemonProcess(command.spawn().unwrap());
        let fixture = Self {
            sandbox,
            workspace: workspace.canonicalize().unwrap(),
            store,
            _daemon: daemon,
        };
        fixture.connect().await;
        fixture
    }

    /// A new client, once the daemon answers.
    async fn connect(&self) -> Client {
        self.sandbox
            .poll_until("the daemon to answer", DEADLINE, async || {
                let stream = UnixStream::connect(self.sandbox.socket()).await.ok()?;
                let info = ClientInfo {
                    name: "daemon-auth-test".into(),
                    version: "0".into(),
                };
                Client::handshake(stream, info)
                    .await
                    .ok()
                    .map(|(client, _, _)| client)
            })
            .await
    }

    async fn new_session(&self, client: &Client) -> SessionId {
        client
            .request::<SessionNew>(&SessionNewParams {
                cwd: self.workspace.clone(),
                environment: self.sandbox.client_environment(),
            })
            .await
            .unwrap()
            .session
    }

    /// Prompts `session` and waits for its turn to end; how it ended, or
    /// the error a turn that ended before it saved the prompt answers.
    async fn prompt(&self, client: &Client, session: &SessionId) -> Result<JobState, RpcError> {
        client
            .request::<SessionPrompt>(&SessionPromptParams {
                session: session.clone(),
                text: "hello".into(),
                timeout_ms: None,
                environment: self.sandbox.client_environment(),
            })
            .await
            .map_err(rpc_error)?;
        // The job recorded the turn's start before the prompt answered, so
        // the first settled state is this turn's.
        Ok(self
            .sandbox
            .poll_until("the turn to end", DEADLINE, async || {
                let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
                let job = jobs
                    .jobs
                    .into_iter()
                    .find(|job| job.session.as_ref() == Some(session))
                    .expect("the daemon lists the session's job");
                match job.state {
                    JobState::Queued | JobState::Running | JobState::WaitingForInput => None,
                    state => Some(state),
                }
            })
            .await)
    }

    fn saved_key(&self) -> Option<String> {
        self.store.get_secret(KEY_ACCOUNT).unwrap()
    }
}

fn rpc_error(error: ClientError) -> RpcError {
    match error {
        ClientError::Rpc { error, .. } => error,
        error => panic!("the request failed outside the protocol: {error}"),
    }
}

async fn save_key(client: &Client, key: &str) -> Result<AuthKeyResponse, RpcError> {
    client
        .request::<AuthKey>(&AuthKeyParams {
            auth: AUTH.into(),
            key: key.into(),
        })
        .await
        .map_err(rpc_error)
}

async fn logout(client: &Client) -> Result<AuthLogoutResponse, RpcError> {
    client
        .request::<AuthLogout>(&AuthLogoutParams { auth: AUTH.into() })
        .await
        .map_err(rpc_error)
}

/// The error code and data of `result`'s error.
fn error_of<T: std::fmt::Debug>(result: Result<T, RpcError>) -> (Option<ErrorCode>, Option<Value>) {
    let error = result.expect_err("the daemon refuses");
    (error.error_code(), error.data)
}

// Covers: a key saved by one client reaches another client's running
// session at its next prompt, a key the provider refuses is not saved over
// the one in the store and does not leak into the error, and a logout makes the running session's next
// prompt fail as an authentication failure even though the provider still
// accepts the key the session last read (#1662).
// Owner: daemon credential methods and the session credential generation,
// over the real socket and HTTP; no fixture provider stands in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_saved_or_deleted_by_one_client_applies_to_another_clients_session() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = Fixture::start(&valid, &[]).await;
    let (first, second) = (fixture.connect().await, fixture.connect().await);

    assert_eq!(
        save_key(&first, FIRST_KEY).await,
        Ok(AuthKeyResponse { unchecked: None })
    );
    let session = fixture.new_session(&first).await;
    assert_eq!(
        fixture.prompt(&first, &session).await,
        Ok(JobState::Completed),
        "log:\n{}",
        fixture.sandbox.daemon_log()
    );

    *valid.lock().unwrap() = SECOND_KEY.to_owned();
    // The provider quotes the key it refuses; the whole error shows none of
    // it.
    assert_eq!(
        (save_key(&second, WRONG_KEY).await, fixture.saved_key()),
        (
            Err(RpcError::new(ErrorCode::KeyRefused, "mock refused the key")),
            Some(FIRST_KEY.into())
        )
    );
    assert_eq!(
        save_key(&second, SECOND_KEY).await,
        Ok(AuthKeyResponse { unchecked: None })
    );
    assert_eq!(
        fixture.prompt(&first, &session).await,
        Ok(JobState::Completed),
        "the session renews to the saved key; log:\n{}",
        fixture.sandbox.daemon_log()
    );

    assert_eq!(logout(&second).await, Ok(AuthLogoutResponse {}));
    let failed = TurnOutcome::Failed {
        error: TurnError {
            kind: TurnErrorKind::Authentication,
            message: "authentication failed for mock (auth: mock-api-key); run /login mock".into(),
        },
    };
    assert_eq!(
        (
            fixture.saved_key(),
            error_of(fixture.prompt(&first, &session).await)
        ),
        (
            None,
            (
                Some(ErrorCode::TurnEnded),
                Some(serde_json::to_value(failed).unwrap())
            )
        ),
        "log:\n{}",
        fixture.sandbox.daemon_log()
    );
}

// Covers: a key in the daemon's environment wins over a saved one, so both
// methods refuse, naming the variable, and leave the saved key alone.
// Owner: daemon credential methods.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_in_the_daemons_environment_refuses_sign_in_and_out() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = Fixture::start(&valid, &[(KEY_VARIABLE, SECOND_KEY)]).await;
    fixture.store.set_secret(KEY_ACCOUNT, FIRST_KEY).unwrap();
    let client = fixture.connect().await;

    let overridden = (
        Some(ErrorCode::EnvironmentOverride),
        Some(
            serde_json::to_value(EnvironmentOverride {
                variable: KEY_VARIABLE.into(),
            })
            .unwrap(),
        ),
    );
    assert_eq!(
        (
            error_of(save_key(&client, FIRST_KEY).await),
            error_of(logout(&client).await),
            fixture.saved_key(),
        ),
        (overridden.clone(), overridden, Some(FIRST_KEY.into()))
    );
}
