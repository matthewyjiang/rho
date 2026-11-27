//! The auth a daemon session starts on, through the real binary and its
//! socket, as rho-ui opens a session for its draft: a config that names a
//! provider but no auth starts on the provider's auth that has
//! credentials, while an auth the config names stays as named, signed in
//! or not.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{ConfigGet, ConfigSet, SessionAttach, SessionDetach, SessionNew},
    Client, ConfigGetParams, ConfigLayer, ConfigNamespace, ConfigSetParams, ModelSelection,
    SessionAttachParams, SessionDetachParams, SessionNewParams, SessionSetup, SettingChange,
    SettingKey, SettingValue,
};
use rho_providers::{
    credentials::{save_xai_tokens, CredentialStore, FileCredentialStore, XaiTokens},
    provider::XAI_API_KEY_ACCOUNT,
};

use super::sessions::{settled_job, Sandbox};

/// SpaceXAI's two auth profiles.
const KEY: &str = "xai-api-key";
const OAUTH: &str = "xai-oauth";

/// A config on `provider`'s `grok-4.7` with `auth` set to `auth`, or unset.
fn config(provider: &str, auth: Option<&str>) -> String {
    let auth = auth
        .map(|auth| format!("auth = \"{auth}\"\n"))
        .unwrap_or_default();
    format!(
        "[model]\nprovider = \"{provider}\"\nmodel = \"grok-4.7\"\n{auth}\n[web_search]\nmode = \"off\"\n"
    )
}

/// The auth profile a session `client` opens now reports.
async fn new_session_auth(sandbox: &Sandbox, client: &Client) -> String {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment: sandbox.client_environment(),
            agent: None,
            setup: SessionSetup::default(),
            parent: None,
            grant: None,
            worktree: false,
        })
        .await
        .unwrap()
        .options
        .model
        .auth
}

/// A sandbox with `config` and the credentials of `signed_in` saved in its
/// file store, as `rho login` would leave them.
fn sandbox(config: &str, signed_in: &[&str]) -> Sandbox {
    let sandbox = Sandbox::new();
    std::fs::write(&sandbox.isolated.config_path, config).unwrap();
    let store = FileCredentialStore::with_rho_home(sandbox.isolated.rho_home()).unwrap();
    for auth in signed_in {
        match *auth {
            KEY => store
                .set_secret(XAI_API_KEY_ACCOUNT, "fixture-key-not-a-real-credential")
                .unwrap(),
            OAUTH => save_xai_tokens(
                &store,
                &XaiTokens {
                    access_token: "fixture-token-not-a-real-credential".into(),
                    refresh_token: None,
                    expires_at_unix: None,
                    id_token: None,
                },
            )
            .unwrap(),
            other => panic!("no fixture credential for {other}"),
        }
    }
    sandbox
}

// Covers: the auth profile a new daemon session reports in its options,
// which rho-ui's sign-in card reads: with no auth in config, the
// provider's auth with credentials, by the selection rules (a key first
// when both are saved, the provider's default when none is); a configured
// auth stays as configured even when only the other one is signed in, as
// does the auth a retired provider name such as `xai-oauth` carries.
// Owner: session starting auth (config auth origin, settled config).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_session_starts_on_the_auth_with_credentials_unless_config_names_one() {
    let cases: [(&str, Option<&str>, &[&str], &str); 6] = [
        ("xai", None, &[OAUTH], OAUTH),
        ("xai", None, &[KEY], KEY),
        ("xai", None, &[KEY, OAUTH], KEY),
        ("xai", None, &[], KEY),
        ("xai", Some(KEY), &[OAUTH], KEY),
        ("xai-oauth", None, &[KEY], OAUTH),
    ];
    let mut started = Vec::new();
    for (provider, auth, signed_in, _) in cases {
        let sandbox = sandbox(&config(provider, auth), signed_in);
        let client = sandbox.start_daemon("fixed").await;
        let opened = new_session_auth(&sandbox, &client).await;
        started.push((provider, auth, signed_in, opened));
    }

    assert_eq!(
        started,
        cases.map(|(provider, auth, signed_in, expected)| {
            (provider, auth, signed_in, expected.to_owned())
        })
    );
}

// Covers: picking the conversation model in settings (rho-ui's Settings
// model picker, `rho/config/set`) while signed in only with OAuth writes
// no auth into `config.toml`, so the next session starts on OAuth rather
// than on the provider's default key, which the pick used to pin.
// Owner: config model write (`model_from_wire`) and session starting auth.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_picked_in_settings_starts_sessions_on_the_auth_with_credentials() {
    let sandbox = sandbox("[web_search]\nmode = \"off\"\n", &[OAUTH]);
    let client = sandbox.start_daemon("fixed").await;
    let state = client
        .request::<ConfigGet>(&ConfigGetParams {
            namespace: ConfigNamespace::Agent,
        })
        .await
        .unwrap();
    client
        .request::<ConfigSet>(&ConfigSetParams {
            namespace: ConfigNamespace::Agent,
            layer: ConfigLayer::Global,
            revision: state.revision,
            writer: None,
            changes: vec![SettingChange::Set {
                key: SettingKey::new("model"),
                value: SettingValue::Model(ModelSelection::Rho {
                    provider: "xai".into(),
                    model: "grok-4.7".into(),
                }),
            }],
        })
        .await
        .unwrap();

    let written = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();
    let opened = new_session_auth(&sandbox, &client).await;

    assert_eq!(
        (written.contains("auth"), opened.as_str()),
        (false, OAUTH),
        "config.toml:\n{written}"
    );
}

// Covers: a saved session that followed the config reopens, after the
// daemon unloaded it, on the auth with credentials, as a new one starts.
// Owner: session starting auth (reopen).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reopened_session_starts_on_the_auth_with_credentials() {
    let sandbox = sandbox(&config("xai", None), &[OAUTH]);
    let client = sandbox
        .start_daemon_with("fixed", &["--unload-idle-after", "200ms"])
        .await;
    let session = sandbox.new_session(&client).await;
    // Saved, so the unload has a file to reopen.
    sandbox.prompt(&client, &session).await.unwrap();
    settled_job(&sandbox, &client, &session).await;
    let attach = SessionAttachParams {
        session: session.clone(),
        after: None,
        environment: sandbox.client_environment(),
        setup: SessionSetup::default(),
    };
    let (watcher, _) = sandbox.connect_following().await;
    watcher.request::<SessionAttach>(&attach).await.unwrap();
    watcher
        .request::<SessionDetach>(&SessionDetachParams {
            session: session.clone(),
        })
        .await
        .unwrap();
    sandbox.wait_unloaded(&session, /*times*/ 1).await;

    let reopened = sandbox
        .connect()
        .await
        .request::<SessionAttach>(&attach)
        .await
        .unwrap();

    assert_eq!(reopened.options.model.auth, OAUTH);
}
