//! Signing the daemon in and out over its socket (#1662): a key one client
//! saves or deletes applies to another client's running session from its
//! next prompt, against a provider that checks every key over real HTTP.
//! Also the credential store the daemon reports and a client chooses.

use std::{
    process::Command,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{AuthChooseStore, AuthKey, AuthLogout, JobList, SessionPrompt, Status},
    AuthChooseStoreParams, AuthChooseStoreResponse, AuthKeyParams, AuthKeyResponse,
    AuthLogoutParams, AuthLogoutResponse, Client, ClientEnvironment, ClientError, ContentBlock,
    CredentialStoreKind, CredentialStoreStatus, EnvironmentOverride, ErrorCode, JobListParams,
    JobState, RpcError, SessionId, SessionPromptParams, StatusParams, StoreSelectedBy, TurnError,
    TurnErrorKind, TurnOutcome,
};
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use serde_json::Value;

use super::{key_checking_provider::spawn_provider, sessions::Sandbox};

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
    sandbox: Sandbox,
    store: FileCredentialStore,
}

impl Fixture {
    /// Starts the daemon with the variables `environment` set.
    async fn start(valid: &Arc<Mutex<String>>, environment: &[(&str, &str)]) -> Self {
        Self::start_with(valid, |command| {
            command.envs(environment.iter().copied());
        })
        .await
    }

    /// Starts the daemon with its command changed by `configure`.
    async fn start_with(valid: &Arc<Mutex<String>>, configure: impl FnOnce(&mut Command)) -> Self {
        let address = spawn_provider(Arc::clone(valid));
        let sandbox = Sandbox::new();
        std::fs::write(
            &sandbox.isolated.config_path,
            format!(
                r#"[model]
provider = "mock"
model = "mock-model"
auth = "{AUTH}"

[providers.custom.mock]
base_url = "http://{address}/v1"

[behavior]

[web_search]
mode = "off"
"#
            ),
        )
        .unwrap();
        let store = FileCredentialStore::with_rho_home(sandbox.isolated.rho_home()).unwrap();
        let mut command = sandbox.rho("fixed");
        // The session must reach the real provider, not the fixture one.
        command.env_remove("RHO_AUTOMATION_TEST_MODE");
        configure(&mut command);
        sandbox.spawn_daemon(command, &[]).await;
        Self { sandbox, store }
    }

    /// Prompts `session` and waits for its turn to end; how it ended, or
    /// the error a turn that ended before it saved the prompt answers.
    async fn prompt(&self, client: &Client, session: &SessionId) -> Result<JobState, RpcError> {
        client
            .request::<SessionPrompt>(&SessionPromptParams {
                session: session.clone(),
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                timeout_ms: None,
                environment: self.sandbox.client_environment(),
                idempotency_key: None,
                grant: None,
            })
            .await
            .map_err(rpc_error)?;
        // The job recorded the turn's start before the prompt answered, so
        // the first settled state is this turn's.
        Ok(self
            .sandbox
            .poll_until("the turn to end", async || {
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

/// Saves `key` for a client that sends no environment, so to the daemon's
/// own store.
async fn save_key(client: &Client, key: &str) -> Result<AuthKeyResponse, RpcError> {
    save_key_from(client, key, ClientEnvironment::default()).await
}

/// Saves `key` for a client whose environment is `environment`.
async fn save_key_from(
    client: &Client,
    key: &str,
    environment: ClientEnvironment,
) -> Result<AuthKeyResponse, RpcError> {
    client
        .request::<AuthKey>(&AuthKeyParams {
            auth: AUTH.into(),
            key: key.into(),
            environment,
        })
        .await
        .map_err(rpc_error)
}

/// Signs out a client that sends no environment.
async fn logout(client: &Client) -> Result<AuthLogoutResponse, RpcError> {
    logout_from(client, ClientEnvironment::default()).await
}

/// Signs out a client whose environment is `environment`.
async fn logout_from(
    client: &Client,
    environment: ClientEnvironment,
) -> Result<AuthLogoutResponse, RpcError> {
    client
        .request::<AuthLogout>(&AuthLogoutParams {
            auth: AUTH.into(),
            environment,
        })
        .await
        .map_err(rpc_error)
}

async fn store_status(client: &Client) -> CredentialStoreStatus {
    client
        .request::<Status>(&StatusParams {})
        .await
        .unwrap()
        .credential_store
}

async fn choose_store(
    client: &Client,
    store: CredentialStoreKind,
) -> Result<AuthChooseStoreResponse, RpcError> {
    client
        .request::<AuthChooseStore>(&AuthChooseStoreParams {
            store,
            environment: ClientEnvironment::default(),
        })
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
    let (first, second) = (
        fixture.sandbox.connect().await,
        fixture.sandbox.connect().await,
    );

    assert_eq!(
        save_key(&first, FIRST_KEY).await,
        Ok(AuthKeyResponse { unchecked: None })
    );
    let session = fixture.sandbox.new_session(&first).await;
    assert_eq!(
        fixture.prompt(&first, &session).await,
        Ok(JobState::Completed),
        "log:\n{}",
        fixture.sandbox.isolated.daemon_log()
    );

    *valid.lock().unwrap() = SECOND_KEY.to_owned();
    // The provider quotes the key it refuses; the whole error shows none of
    // it.
    assert_eq!(
        (save_key(&second, WRONG_KEY).await, fixture.saved_key()),
        (
            Err(RpcError::new(
                ErrorCode::KeyRefused,
                "mock refused the key: its models endpoint answered HTTP 401 Unauthorized"
            )),
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
        fixture.sandbox.isolated.daemon_log()
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
        fixture.sandbox.isolated.daemon_log()
    );
}

// Covers: a key in a client's environment wins over a saved one in that
// client's sessions, so both methods refuse that client, naming the
// variable, and leave the saved key alone. The same variable in the
// daemon's own environment refuses nobody: a client without it signs in
// and out (#1779).
// Owner: daemon credential methods.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_in_a_clients_environment_refuses_its_sign_in_and_out() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = Fixture::start(&valid, &[(KEY_VARIABLE, SECOND_KEY)]).await;
    fixture.store.set_secret(KEY_ACCOUNT, FIRST_KEY).unwrap();
    let client = fixture.sandbox.connect().await;
    let exporting = || -> ClientEnvironment {
        [(KEY_VARIABLE.to_owned(), SECOND_KEY.to_owned())]
            .into_iter()
            .collect()
    };

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
            error_of(save_key_from(&client, FIRST_KEY, exporting()).await),
            error_of(logout_from(&client, exporting()).await),
            fixture.saved_key(),
        ),
        (overridden.clone(), overridden, Some(FIRST_KEY.into()))
    );
    assert_eq!(
        (
            save_key(&client, FIRST_KEY).await,
            logout(&client).await,
            fixture.saved_key(),
        ),
        (
            Ok(AuthKeyResponse { unchecked: None }),
            Ok(AuthLogoutResponse {}),
            None
        ),
        "log:\n{}",
        fixture.sandbox.isolated.daemon_log()
    );
}

// Covers: a daemon with no session D-Bus, as one a client started from an
// SSH login, reports that it cannot reach the OS store nothing chose for
// it, and fails closed: it refuses a key, and refuses choosing the OS
// store, keeping nothing in the file store meanwhile. Once a client
// chooses the file store, the status says so and why, keys land there, and
// a session signs in with them (#1662).
// Owner: daemon credential store status and choice, over the real socket;
// the sandbox's empty XDG_RUNTIME_DIR is the missing session bus.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_without_a_session_bus_fails_closed_until_a_client_chooses_the_file_store() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    // Nothing chooses the store: not the sandbox's environment, nor its config.
    let fixture = Fixture::start_with(&valid, |command| {
        command.env_remove("RHO_CREDENTIAL_STORE");
    })
    .await;
    let client = fixture.sandbox.connect().await;

    let headless = store_status(&client).await;
    assert_eq!(
        (
            headless.store,
            headless.selected_by,
            headless.unreachable.is_some()
        ),
        (CredentialStoreKind::Os, StoreSelectedBy::Default, true),
        "{headless:?}"
    );
    assert_eq!(
        (
            error_of(save_key(&client, FIRST_KEY).await).0,
            error_of(choose_store(&client, CredentialStoreKind::Os).await).0,
            fixture.saved_key(),
        ),
        (
            Some(ErrorCode::CredentialStoreUnavailable),
            Some(ErrorCode::CredentialStoreUnavailable),
            None
        )
    );

    let chosen = CredentialStoreStatus {
        store: CredentialStoreKind::File,
        selected_by: StoreSelectedBy::Config,
        unreachable: None,
    };
    assert_eq!(
        choose_store(&client, CredentialStoreKind::File).await,
        Ok(AuthChooseStoreResponse {
            credential_store: chosen.clone()
        })
    );
    assert_eq!(store_status(&client).await, chosen);
    assert_eq!(
        save_key(&client, FIRST_KEY).await,
        Ok(AuthKeyResponse { unchecked: None })
    );
    let session = fixture.sandbox.new_session(&client).await;
    assert_eq!(
        (fixture.saved_key(), fixture.prompt(&client, &session).await),
        (Some(FIRST_KEY.into()), Ok(JobState::Completed)),
        "log:\n{}",
        fixture.sandbox.isolated.daemon_log()
    );
}

// Covers: RHO_CREDENTIAL_STORE in the daemon's environment chose its store,
// so the status says so, and choosing a store is refused, naming the
// variable, since config would change nothing.
// Owner: daemon credential store status and choice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_the_daemons_environment_chose_cannot_be_replaced() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    // The sandbox sets RHO_CREDENTIAL_STORE=file for every rho it starts.
    let fixture = Fixture::start(&valid, &[]).await;
    let client = fixture.sandbox.connect().await;

    assert_eq!(
        (
            store_status(&client).await,
            error_of(choose_store(&client, CredentialStoreKind::File).await),
        ),
        (
            CredentialStoreStatus {
                store: CredentialStoreKind::File,
                selected_by: StoreSelectedBy::Environment,
                unreachable: None,
            },
            (
                Some(ErrorCode::EnvironmentOverride),
                Some(
                    serde_json::to_value(EnvironmentOverride {
                        variable: "RHO_CREDENTIAL_STORE".into(),
                    })
                    .unwrap()
                )
            )
        )
    );
}
