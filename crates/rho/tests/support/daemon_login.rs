//! Interactive logins the daemon runs and relays to the client that started
//! them (#1662), through the real binary and its socket, against the
//! offline login fixture (`RHO_AUTOMATION_TEST_LOGIN`): a login to
//! `xai-oauth` serves a loopback page and shows it as a device-code
//! prompt, and a GET of the page approves it with placeholder tokens.
//! Ollama Cloud's device-key login needs no fixture, since it calls nobody.

use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
};

use base64::{engine::general_purpose, Engine as _};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{AuthList, AuthLogin, AuthLoginCancel, SessionAttach, SessionNew},
    AuthListParams, AuthLoginCancelParams, AuthLoginFinishedParams, AuthLoginParams, AuthStatus,
    Client, ClientEnvironment, DaemonNotification, ErrorCode, LoginFlow, LoginId, LoginOutcome,
    LoginPrompt, Notifications, SessionAttachParams, SessionNewParams, SessionOptions,
    SessionSetup,
};
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use super::{
    configure::next_options,
    sessions::{Sandbox, DEADLINE},
};

/// The auth profile the fixture signs in to.
const AUTH: &str = "xai-oauth";
/// The device code the fixture's prompt shows.
const FIXTURE_CODE: &str = "FIXTURE-CODE";
/// The access token the fixture's approval saves.
const FIXTURE_TOKEN: &str = "automation-test-login-token";
/// Where the credential store keeps the SpaceXAI tokens.
const TOKENS_ACCOUNT: &str = "provider:xai:tokens";

/// `rho` with the login fixture on and no SpaceXAI credential in its
/// environment, so only a saved login signs it in.
fn rho(sandbox: &Sandbox) -> Command {
    let mut command = sandbox.rho("fixed");
    command
        .env("RHO_AUTOMATION_TEST_LOGIN", "1")
        .env_remove("XAI_API_KEY")
        .env_remove("XAI_ACCESS_TOKEN");
    command
}

async fn start_daemon(sandbox: &Sandbox) -> Client {
    sandbox.spawn_daemon(rho(sandbox), &[]).await.1
}

/// Starts a login to [`AUTH`]; its id, or the error code that refused it.
async fn start_login(client: &Client) -> Result<LoginId, Option<ErrorCode>> {
    client
        .request::<AuthLogin>(&AuthLoginParams {
            auth: AUTH.into(),
            flow: Some(LoginFlow::DeviceCode),
            environment: Default::default(),
        })
        .await
        .map(|response| response.login)
        .map_err(|error| error.error_code())
}

/// What `rho/auth/list` says of `auth` to a client with `environment`.
async fn status(client: &Client, auth: &str, environment: ClientEnvironment) -> AuthStatus {
    client
        .request::<AuthList>(&AuthListParams { environment })
        .await
        .unwrap()
        .auths
        .into_iter()
        .find(|summary| summary.auth == auth)
        .unwrap_or_else(|| panic!("the daemon lists {auth}"))
        .status
}

/// Ollama Cloud's device-key profile. Its login is offline: it creates the
/// key file when there is none and shows a URL that names the public key.
const DEVICE_AUTH: &str = "ollama-cloud-device";
/// The variable that names the directory a client keeps its device key in.
const KEY_DIR_ENV: &str = "OLLAMA_DEVICE_KEY_DIR";

/// The environment of a client of `sandbox` that keeps its Ollama device
/// key in `key_dir`.
fn environment_keeping_key_in(sandbox: &Sandbox, key_dir: &Path) -> ClientEnvironment {
    sandbox
        .client_environment()
        .into_iter()
        .chain([(
            KEY_DIR_ENV.to_owned(),
            key_dir.to_string_lossy().into_owned(),
        )])
        .collect()
}

/// Logs a new client with `environment` in to [`DEVICE_AUTH`]: the public
/// key its prompt asks the user to approve, and how the login ended.
async fn device_login(sandbox: &Sandbox, environment: ClientEnvironment) -> (String, LoginOutcome) {
    let (client, mut notifications) = sandbox.connect_following().await;
    let login = client
        .request::<AuthLogin>(&AuthLoginParams {
            auth: DEVICE_AUTH.into(),
            flow: None,
            environment,
        })
        .await
        .unwrap()
        .login;
    let prompt = next_prompt(&mut notifications, &login).await;
    let url = url::Url::parse(&prompt.url).unwrap();
    let (_, key) = url
        .query_pairs()
        .find(|(name, _)| name == "key")
        .unwrap_or_else(|| panic!("the connect URL names a key: {url}"));
    let key = general_purpose::URL_SAFE_NO_PAD
        .decode(key.as_bytes())
        .unwrap();
    (
        String::from_utf8(key).unwrap(),
        next_finished(&mut notifications).await.outcome,
    )
}

/// The public key in `key_dir`, when a device key was created there.
fn public_key_in(key_dir: &Path) -> Option<String> {
    std::fs::read_to_string(key_dir.join("id_ed25519.pub"))
        .ok()
        .map(|key| key.trim().to_owned())
}

/// The next login notification on `notifications`.
async fn next_login(notifications: &mut Notifications) -> DaemonNotification {
    tokio::time::timeout(DEADLINE, notifications.recv())
        .await
        .expect("the daemon relays the login")
        .expect("the connection stays open")
}

async fn next_prompt(notifications: &mut Notifications, login: &LoginId) -> LoginPrompt {
    match next_login(notifications).await {
        DaemonNotification::LoginPrompt(params) if &params.login == login => params.prompt,
        other => panic!("expected login {login}'s prompt, got {other:?}"),
    }
}

async fn next_finished(notifications: &mut Notifications) -> AuthLoginFinishedParams {
    match next_login(notifications).await {
        DaemonNotification::LoginFinished(params) => params,
        other => panic!("expected a login's end, got {other:?}"),
    }
}

/// Approves the fixture's login, as a person opening its URL would.
async fn approve(url: &str) {
    let address = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(address, _)| address)
        .unwrap_or_else(|| panic!("the fixture's URL is a loopback page: {url}"));
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /approve HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    // The page closes once the login took the approval.
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
}

/// The access token saved for SpaceXAI in the sandbox's file store.
fn saved_token(sandbox: &Sandbox) -> Option<String> {
    let store = FileCredentialStore::with_rho_home(sandbox.isolated.rho_home()).unwrap();
    let tokens = store.get_secret(TOKENS_ACCOUNT).unwrap()?;
    let tokens: Value = serde_json::from_str(&tokens).unwrap();
    tokens["access_token"].as_str().map(str::to_owned)
}

/// The providers of the models `after` offers that `before` did not, and
/// how many `before` offered that `after` dropped.
fn models_gained(before: &SessionOptions, after: &SessionOptions) -> (BTreeSet<String>, usize) {
    let models = |options: &SessionOptions| {
        options
            .model
            .available
            .iter()
            .map(|choice| choice.model.clone())
            .collect::<Vec<_>>()
    };
    let (before, after) = (models(before), models(after));
    let gained = after
        .iter()
        .filter(|model| !before.contains(model))
        .map(|model| model.provider.clone())
        .collect();
    let dropped = before.iter().filter(|model| !after.contains(model)).count();
    (gained, dropped)
}

// Covers: a client's login runs in the daemon, which relays its device-code
// prompt to that client and, once approved, saves the tokens and reports
// success; a session another client runs gains the provider's models
// without a restart (`options_changed`), and `rho/auth/list` reports the
// profile signed in (#1662 acceptance 2).
// Owner: daemon login relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relayed_login_signs_in_a_running_session() {
    let sandbox = Sandbox::new();
    let client = start_daemon(&sandbox).await;
    let opened = client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment: sandbox.client_environment(),
            agent: None,
            setup: SessionSetup::default(),
            parent: None,
            grant: None,
        })
        .await
        .unwrap();
    let (session, before) = (opened.session, opened.options);
    let (watcher, mut events) = sandbox.connect_following().await;
    watcher
        .request::<SessionAttach>(&SessionAttachParams {
            session: session.clone(),
            after: None,
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        })
        .await
        .unwrap();
    let (login_client, mut notifications) = sandbox.connect_following().await;
    let signed_out = status(&login_client, AUTH, Default::default()).await;

    let login = start_login(&login_client).await.unwrap();
    let prompt = next_prompt(&mut notifications, &login).await;
    approve(&prompt.url).await;
    let finished = next_finished(&mut notifications).await;
    let after = next_options(&mut events, &session).await;

    assert_eq!(
        (
            signed_out,
            (prompt.user_code.as_deref(), prompt.url_with_code.as_deref()),
            finished,
            models_gained(&before, &after),
            status(&login_client, AUTH, Default::default()).await,
            saved_token(&sandbox),
        ),
        (
            AuthStatus::SignedOut,
            (Some(FIXTURE_CODE), None),
            AuthLoginFinishedParams {
                login,
                outcome: LoginOutcome::Succeeded,
            },
            (BTreeSet::from(["xai".to_owned()]), 0),
            AuthStatus::SignedIn,
            Some(FIXTURE_TOKEN.to_owned()),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: the Ollama device key is each client's, kept in the directory its
// own `OLLAMA_DEVICE_KEY_DIR` names. A daemon whose environment names a
// directory of its own, as one inherited from the client that started it
// would, creates no key there: each of two clients is asked to approve a
// key the daemon created in that client's directory, the two keys differ,
// and after the first client's login the second, which has no key yet, is
// still signed out (#1789).
// Owner: daemon login relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_clients_device_login_uses_the_key_in_its_own_directory() {
    let sandbox = Sandbox::new();
    let [daemon_dir, first_dir, second_dir] = [(); 3].map(|()| tempfile::tempdir().unwrap());
    let mut daemon = rho(&sandbox);
    daemon.env(KEY_DIR_ENV, daemon_dir.path());
    let client = sandbox.spawn_daemon(daemon, &[]).await.1;
    let first = || environment_keeping_key_in(&sandbox, first_dir.path());
    let second = || environment_keeping_key_in(&sandbox, second_dir.path());

    let (first_key, first_outcome) = device_login(&sandbox, first()).await;
    let after_first = (
        status(&client, DEVICE_AUTH, first()).await,
        status(&client, DEVICE_AUTH, second()).await,
        public_key_in(second_dir.path()),
    );
    let (second_key, second_outcome) = device_login(&sandbox, second()).await;

    assert_ne!(first_key, second_key);
    assert_eq!(
        (
            (first_outcome, Some(first_key)),
            after_first,
            (second_outcome, Some(second_key)),
            status(&client, DEVICE_AUTH, second()).await,
            public_key_in(daemon_dir.path()),
        ),
        (
            (LoginOutcome::Succeeded, public_key_in(first_dir.path())),
            (AuthStatus::SignedIn, AuthStatus::SignedOut, None),
            (LoginOutcome::Succeeded, public_key_in(second_dir.path())),
            AuthStatus::SignedIn,
            None,
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a second login to a profile while one runs is refused as busy; a
// cancelled login ends `cancelled`, saves nothing, and frees the profile;
// a login whose client disconnects is cancelled, freeing it too.
// Owner: daemon login relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_login_ends_when_cancelled_or_its_client_leaves() {
    let sandbox = Sandbox::new();
    let client = start_daemon(&sandbox).await;
    let (first, mut notifications) = sandbox.connect_following().await;
    let login = start_login(&first).await.unwrap();
    next_prompt(&mut notifications, &login).await;
    let busy = start_login(&client).await.err();

    first
        .request::<AuthLoginCancel>(&AuthLoginCancelParams {
            login: login.clone(),
        })
        .await
        .unwrap();
    let cancelled = next_finished(&mut notifications).await;
    let cancelled_again = first
        .request::<AuthLoginCancel>(&AuthLoginCancelParams {
            login: login.clone(),
        })
        .await
        .map_err(|error| error.error_code())
        .err();

    // The profile is free as soon as its client hears the end.
    let (leaver, leaver_notifications) = sandbox.connect_following().await;
    start_login(&leaver)
        .await
        .expect("a cancelled login frees its profile");
    drop((leaver, leaver_notifications));
    sandbox
        .poll_until(
            "the daemon to cancel the login whose client left",
            async || match start_login(&client).await {
                Ok(login) => Some(login),
                Err(Some(ErrorCode::LoginBusy)) => None,
                Err(code) => panic!("a login was refused with {code:?}"),
            },
        )
        .await;

    assert_eq!(
        (busy, cancelled, cancelled_again, saved_token(&sandbox)),
        (
            Some(Some(ErrorCode::LoginBusy)),
            AuthLoginFinishedParams {
                login,
                outcome: LoginOutcome::Cancelled,
            },
            Some(Some(ErrorCode::LoginNotFound)),
            None,
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: with no daemon running, `rho login` starts one, prints the prompt
// the daemon relays, and exits 0 once the login is approved, with the
// tokens saved where the daemon's sessions read them.
// Owner: rho login CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rho_login_signs_in_through_a_daemon_it_starts() {
    // Dropping the sandbox stops the daemon `rho login` starts.
    let sandbox = Sandbox::new();
    let mut login = rho(&sandbox)
        // No browser to open, so nothing launches on the test machine.
        .env("SSH_CONNECTION", "127.0.0.1 1 127.0.0.1 2")
        .args(["login", AUTH])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(login.stderr.take().unwrap());
    let printed_url = tokio::task::spawn_blocking(move || {
        let mut printed = String::new();
        loop {
            let mut line = String::new();
            let read = stderr.read_line(&mut line).unwrap();
            assert!(read > 0, "rho login printed no URL:\n{printed}");
            printed.push_str(&line);
            if line.starts_with("http://127.0.0.1:") {
                return (line.trim_end().to_owned(), stderr);
            }
        }
    });
    // Each read ends when `rho login` writes or exits; killing it on the
    // deadline ends a read that would otherwise wait forever.
    let Ok(printed_url) = tokio::time::timeout(DEADLINE, printed_url).await else {
        login.kill().unwrap();
        panic!(
            "rho login printed no URL\nlog:\n{}",
            sandbox.isolated.daemon_log()
        );
    };
    let (url, mut stderr) = printed_url.unwrap();
    approve(&url).await;
    let rest = tokio::task::spawn_blocking(move || {
        let mut rest = String::new();
        std::io::Read::read_to_string(&mut stderr, &mut rest).unwrap();
        rest
    });
    let Ok(rest) = tokio::time::timeout(DEADLINE, rest).await else {
        login.kill().unwrap();
        panic!(
            "rho login did not exit\nlog:\n{}",
            sandbox.isolated.daemon_log()
        );
    };
    let (status, rest) = (login.wait().unwrap(), rest.unwrap());

    assert_eq!(
        (status.code(), saved_token(&sandbox)),
        (Some(0), Some(FIXTURE_TOKEN.to_owned())),
        "stderr:\n{rest}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}
