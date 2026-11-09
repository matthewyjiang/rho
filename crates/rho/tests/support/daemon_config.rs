//! Reading and changing settings through the daemon (`rho/config/*`,
//! #1720), through the real binary and its socket: a change is saved to
//! `config.toml`, reaches every client reading the settings and the next
//! turn of a loaded session; a stale or refused change writes nothing; a
//! hand edit is served, and a broken one keeps the last good values in
//! force; a client's own settings outlast the daemon.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{ConfigGet, ConfigRegister, ConfigSet, SessionAttach},
    Applies, Client, ClientError, ClientName, ConfigChangedParams, ConfigError, ConfigGetParams,
    ConfigGetResponse, ConfigLayer, ConfigNamespace, ConfigRegisterParams, ConfigRevision,
    ConfigSetParams, DaemonNotification, ErrorCode, ModelSelection, Notifications, RpcError,
    SessionAttachParams, SessionEvent, SessionId, SessionSetup, SettingChange, SettingEntry,
    SettingKey, SettingKind, SettingSpec, SettingValue, ValueSource,
};
use serde_json::Value;

use super::sessions::{Sandbox, DEADLINE};

async fn get(client: &Client, namespace: ConfigNamespace) -> ConfigGetResponse {
    client
        .request::<ConfigGet>(&ConfigGetParams { namespace })
        .await
        .unwrap()
}

/// Sends `changes` against `revision`; the new revision, or the error that
/// refused them.
async fn set(
    client: &Client,
    namespace: ConfigNamespace,
    revision: &ConfigRevision,
    changes: Vec<SettingChange>,
) -> Result<ConfigRevision, RpcError> {
    client
        .request::<ConfigSet>(&ConfigSetParams {
            namespace,
            layer: ConfigLayer::Global,
            revision: revision.clone(),
            changes,
        })
        .await
        .map(|response| response.revision)
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error,
            error => panic!("set failed outside the protocol: {error}"),
        })
}

fn change(key: &str, value: SettingValue) -> SettingChange {
    SettingChange::Set {
        key: SettingKey::new(key),
        value,
    }
}

fn value(state: &ConfigGetResponse, key: &str) -> SettingEntry {
    state.values[&SettingKey::new(key)].clone()
}

fn global(value: SettingValue) -> SettingEntry {
    SettingEntry {
        value: Some(value),
        source: ValueSource::Global,
    }
}

/// The next notification, which must come within [`DEADLINE`].
async fn next_notification(notifications: &mut Notifications) -> DaemonNotification {
    tokio::time::timeout(DEADLINE, notifications.recv())
        .await
        .expect("the daemon notifies")
        .expect("the connection stays open")
}

/// The tools of the model request the `inspect` fixture answered the next
/// turn with, and the notices the session showed before the turn ended.
async fn next_turn(
    notifications: &mut Notifications,
    session: &SessionId,
) -> (Vec<String>, Vec<String>) {
    let (mut answer, mut notices) = (String::new(), Vec::new());
    loop {
        let DaemonNotification::SessionEvent(params) = next_notification(notifications).await
        else {
            continue;
        };
        assert_eq!(&params.session, session);
        match params.event {
            SessionEvent::AgentMessageChunk { text, .. } => answer.push_str(&text),
            SessionEvent::Notice { text } => notices.push(text),
            SessionEvent::TurnEnded { .. } => break,
            _ => {}
        }
    }
    let request: Value =
        serde_json::from_str(&answer).unwrap_or_else(|error| panic!("{error}: {answer}"));
    let tools = request["tools"]
        .as_array()
        .expect("the request lists its tools")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    (tools, notices)
}

// Covers: a change set through the protocol is saved to `config.toml`,
// keeping the file's comments; every client that read the settings gets
// the changed values as `rho/config/changed`; and a session already loaded
// takes a next-turn setting at its next turn: turning the advisor on with
// its model in one change offers the model the `advisor` tool and tells
// the session's clients (#1720).
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_is_saved_and_reaches_readers_and_a_loaded_sessions_next_turn() {
    let sandbox = Sandbox::new();
    let config_before = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();
    std::fs::write(
        &sandbox.isolated.config_path,
        format!("# kept by hand\n{config_before}"),
    )
    .unwrap();
    let client = sandbox.start_daemon("inspect").await;
    let session = sandbox.new_session(&client).await;
    let (follower, mut events) = sandbox.connect_following().await;
    follower
        .request::<SessionAttach>(&SessionAttachParams {
            session: session.clone(),
            after: None,
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        })
        .await
        .unwrap();
    sandbox.prompt(&client, &session).await.unwrap();
    let (tools, _) = next_turn(&mut events, &session).await;
    assert!(!tools.contains(&"advisor".to_owned()), "tools: {tools:?}");

    let (reader, mut changes) = sandbox.connect_following().await;
    let state = get(&reader, ConfigNamespace::Agent).await;
    assert_eq!(state.errors, []);
    let advisor_model = SettingValue::Model(ModelSelection::Rho {
        provider: "xai".into(),
        model: "grok-fixture".into(),
    });
    let revision = set(
        &client,
        ConfigNamespace::Agent,
        &state.revision,
        vec![
            change("internal_agents.advisor", advisor_model.clone()),
            change("behavior.advisor_mode", SettingValue::Bool(true)),
        ],
    )
    .await
    .unwrap();

    let saved = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();
    assert!(saved.starts_with("# kept by hand\n"), "{saved}");
    assert!(saved.contains("advisor_mode = true"), "{saved}");
    let notified = next_notification(&mut changes).await;
    assert_eq!(
        notified,
        DaemonNotification::ConfigChanged(ConfigChangedParams {
            namespace: ConfigNamespace::Agent,
            revision,
            values: [
                ("behavior.advisor_mode", global(SettingValue::Bool(true))),
                ("internal_agents.advisor", global(advisor_model)),
            ]
            .into_iter()
            .map(|(key, entry)| (SettingKey::new(key), entry))
            .collect(),
            errors: Vec::new(),
        })
    );

    sandbox.prompt(&client, &session).await.unwrap();
    let (tools, notices) = next_turn(&mut events, &session).await;
    assert!(
        tools.contains(&"advisor".to_owned()),
        "tools: {tools:?}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
    assert_eq!(
        notices.len(),
        1,
        "one notice says the advisor is on: {notices:?}"
    );
}

// Covers: a change against a stale revision is refused as a conflict, and
// one with a bad value or an unknown key as invalid params naming them;
// neither touches the file (#1720).
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_or_refused_change_writes_nothing() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let state = get(&client, ConfigNamespace::Agent).await;
    let revision = set(
        &client,
        ConfigNamespace::Agent,
        &state.revision,
        vec![change("behavior.rtk", SettingValue::Bool(false))],
    )
    .await
    .unwrap();
    let saved = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();

    let stale = set(
        &client,
        ConfigNamespace::Agent,
        &state.revision,
        vec![change("behavior.rtk", SettingValue::Bool(true))],
    )
    .await;
    let refused = set(
        &client,
        ConfigNamespace::Agent,
        &revision,
        vec![
            change(
                "compaction.compact_threshold_percent",
                SettingValue::Integer(101),
            ),
            change("display.zen_mode", SettingValue::Bool(true)),
        ],
    )
    .await;
    assert_eq!(
        [stale.map(drop), refused.map(drop)],
        [
            Err(RpcError::new(
                ErrorCode::ConfigConflict,
                "the settings changed since that revision; read them again",
            )),
            Err(RpcError::new(
                ErrorCode::InvalidParams,
                "`compaction.compact_threshold_percent`: must be at most 100; got 101\n`display.zen_mode`: no such setting",
            )),
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&sandbox.isolated.config_path).unwrap(),
        saved
    );
}

// Covers: a hand edit of `config.toml` is served on the next request; one
// that no longer loads is reported in `errors` while the last good values
// stay served and in force, so a new session still opens (#1720).
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hand_edit_is_served_and_a_broken_one_keeps_the_last_good_values() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    sandbox.set_config("supervised", "[output]\nmax_output_bytes = 4096\n");
    let edited = get(&client, ConfigNamespace::Agent).await;
    assert_eq!(
        (
            value(&edited, "output.max_output_bytes"),
            edited.errors.clone()
        ),
        (global(SettingValue::Integer(4096)), Vec::new())
    );

    sandbox.set_config("supervised", "[output]\nmax_output_bytes = 0\n");
    let broken = get(&client, ConfigNamespace::Agent).await;
    assert_eq!(
        (
            value(&broken, "output.max_output_bytes"),
            broken.errors.clone()
        ),
        (
            global(SettingValue::Integer(4096)),
            vec![ConfigError {
                key: Some(SettingKey::new("output.max_output_bytes")),
                message: "must be at least 1; got 0".into(),
            }]
        )
    );
    sandbox.new_session(&client).await;
}

// Covers: a client's registered settings are stored for it and served
// again by a daemon started later, once the client registers again
// (#1720).
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clients_settings_outlast_the_daemon() {
    let sandbox = Sandbox::new();
    let tui = || ConfigNamespace::Client {
        name: ClientName::new("tui").unwrap(),
    };
    let register = async |client: &Client| {
        client
            .request::<ConfigRegister>(&ConfigRegisterParams {
                client: ClientName::new("tui").unwrap(),
                settings: vec![SettingSpec {
                    key: SettingKey::new("zen_mode"),
                    kind: SettingKind::Bool,
                    default: Some(SettingValue::Bool(false)),
                    category: "display".into(),
                    section: None,
                    label: "Zen mode".into(),
                    help: "Show only message text.".into(),
                    applies: Applies::Immediately,
                    overridable_per_project: false,
                }],
            })
            .await
            .unwrap();
    };
    let client = sandbox.start_daemon("fixed").await;
    register(&client).await;
    let state = get(&client, tui()).await;
    set(
        &client,
        tui(),
        &state.revision,
        vec![change("zen_mode", SettingValue::Bool(true))],
    )
    .await
    .unwrap();
    let stop = sandbox
        .rho("fixed")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stop.success(), "log:\n{}", sandbox.isolated.daemon_log());

    let client = sandbox.start_daemon("fixed").await;
    register(&client).await;
    assert_eq!(
        value(&get(&client, tui()).await, "zen_mode"),
        global(SettingValue::Bool(true))
    );
}
