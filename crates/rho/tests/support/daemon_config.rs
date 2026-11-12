//! Reading and changing settings through the daemon (`rho/config/*`,
//! #1720), through the real binary and its socket: a change is saved to
//! `config.toml`, reaches every client reading the settings and the next
//! turn of a loaded session; a stale or refused change writes nothing; a
//! hand edit is served, and a broken one keeps the last good values in
//! force; a client's own settings outlast the daemon. A custom provider
//! added, edited or removed reaches a loaded session at its next turn.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{ConfigGet, ConfigRegister, ConfigSet, SessionAttach, SessionConfigure},
    Applies, Client, ClientError, ClientName, ConfigChangedParams, ConfigError, ConfigGetParams,
    ConfigGetResponse, ConfigLayer, ConfigNamespace, ConfigRegisterParams, ConfigRevision,
    ConfigSetParams, ConfigWriter, DaemonNotification, ErrorCode, ModelSelection, Notifications,
    PermissionMode, RpcError, SessionAttachParams, SessionChange, SessionConfigureParams,
    SessionEvent, SessionId, SessionSetup, SettingChange, SettingEntry, SettingKey, SettingKind,
    SettingSpec, SettingValue, TurnError, TurnErrorKind, TurnOutcome, ValueSource,
};
use serde_json::Value;

use super::{
    scripted_host::{Reply, ScriptedHost},
    sessions::{Sandbox, DEADLINE},
};

async fn get(client: &Client, namespace: ConfigNamespace) -> ConfigGetResponse {
    client
        .request::<ConfigGet>(&ConfigGetParams { namespace })
        .await
        .unwrap()
}

/// Sends `changes` against `revision`, naming no writer; the new revision,
/// or the error that refused them.
async fn set(
    client: &Client,
    namespace: ConfigNamespace,
    revision: &ConfigRevision,
    changes: Vec<SettingChange>,
) -> Result<ConfigRevision, RpcError> {
    set_by(client, None, namespace, revision, changes).await
}

/// [`set`] by `writer`.
async fn set_by(
    client: &Client,
    writer: Option<&str>,
    namespace: ConfigNamespace,
    revision: &ConfigRevision,
    changes: Vec<SettingChange>,
) -> Result<ConfigRevision, RpcError> {
    client
        .request::<ConfigSet>(&ConfigSetParams {
            namespace,
            layer: ConfigLayer::Global,
            revision: revision.clone(),
            writer: writer.map(ConfigWriter::new),
            changes,
        })
        .await
        .map(|response| response.revision)
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error,
            error => panic!("set failed outside the protocol: {error}"),
        })
}

/// The `tui` client's namespace.
fn tui() -> ConfigNamespace {
    ConfigNamespace::Client {
        name: ClientName::new("tui").unwrap(),
    }
}

/// Registers the `tui` client with one setting, `zen_mode`.
async fn register_tui(client: &Client) {
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
    let (answer, notices) = next_turn_reply(notifications, session).await;
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

/// The text `session`'s next turn answered with, and the notices it showed
/// before it ended.
async fn next_turn_reply(
    notifications: &mut Notifications,
    session: &SessionId,
) -> (String, Vec<String>) {
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
    (answer, notices)
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

// Covers: a custom provider added to `config.toml` after a session opened,
// with the session's advisor, title, compaction and permission-classifier
// models on it, reaches each of them from the session's next turn: the
// session takes the providers with those models, and rebuilds what it
// built on the ones it opened with (#1685).
// Owner: daemon next-turn providers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_added_later_reaches_each_internal_agent_of_a_loaded_session() {
    const MODELS: &[&str] = &["advisor", "classifier", "compaction", "title"];
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    let (_follower, mut events) = follow(&sandbox, &session).await;

    let host = ScriptedHost::spawn(MODELS, |_| Reply::Text("answered"));
    let config = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();
    let agent = |id: &str, model: &str| {
        format!(
            "[internal_agents.{id}]\nprovider = \"mock\"\nmodel = \"{model}\"\nauth = \"none\"\n"
        )
    };
    std::fs::write(
        &sandbox.isolated.config_path,
        format!(
            "{config}\n[behavior]\nadvisor_mode = true\n\n{}\n{}\n{}\n{}\n{}",
            agent("advisor", "advisor"),
            agent("permission-classifier", "classifier"),
            agent("compaction", "compaction"),
            agent("session-title", "title"),
            host.provider_table("mock"),
        ),
    )
    .unwrap();
    // Taken from the config as it is now, the classifier model on the
    // provider the session does not have yet.
    client
        .request::<SessionConfigure>(&SessionConfigureParams {
            session: session.clone(),
            change: SessionChange::PermissionMode {
                mode: PermissionMode::Auto,
            },
        })
        .await
        .unwrap();

    // The fixture makes each call; the advisor's reply is the call's result,
    // and the turn's reply names the session.
    sandbox
        .prompt_tool_call(
            &client,
            &session,
            &serde_json::json!({"id": "consult", "name": "advisor", "arguments": {}}),
        )
        .await;
    let (reply, _) = next_turn_reply(&mut events, &session).await;
    assert_eq!(reply, "answered", "log:\n{}", sandbox.isolated.daemon_log());
    // A command Auto asks the classifier about.
    sandbox
        .prompt_tool_call(
            &client,
            &session,
            &serde_json::json!({
                "id": "run",
                "name": "bash",
                "arguments": {"command": "echo classified"},
            }),
        )
        .await;
    next_turn_reply(&mut events, &session).await;
    sandbox
        .prompt_text(&client, &session, "/compact")
        .await
        .unwrap();
    next_turn_reply(&mut events, &session).await;

    let asked = sandbox
        .poll_until("every internal agent to ask the added host", async || {
            let mut asked = host.models();
            asked.sort();
            asked.dedup();
            (asked == MODELS).then_some(asked)
        })
        .await;
    assert_eq!(asked, MODELS);
}

// Covers: a `base_url` edited while a turn runs is not what the rest of
// that turn's requests go to; the session's next turn goes to the edited
// one (#1685).
// Owner: daemon next-turn providers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edited_base_url_reaches_the_next_turn_not_the_running_one() {
    let (asked, mut asked_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = std::sync::Mutex::new(Some(released));
    // Holds the first request's answer, a tool call, until the test
    // releases it, and answers every later one, such as the request after
    // the call's result or the title agent's, `old`.
    let old = ScriptedHost::spawn(&["mock-model"], move |_| {
        let Some(released) = released.lock().unwrap().take() else {
            return Reply::Text("old");
        };
        asked.send(()).unwrap();
        released.recv().unwrap();
        Reply::ToolCall {
            name: "read_file",
            arguments: serde_json::json!({"path": "notes.txt"}),
        }
    });
    let new = ScriptedHost::spawn(&["mock-model"], |_| Reply::Text("new"));
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.workspace.join("notes.txt"), "notes\n").unwrap();
    let client = start_on_custom_host(&sandbox, &old).await;
    let session = sandbox.new_session(&client).await;
    let (_follower, mut events) = follow(&sandbox, &session).await;

    sandbox.prompt(&client, &session).await.unwrap();
    tokio::time::timeout(DEADLINE, asked_rx.recv())
        .await
        .expect("the turn asks the host")
        .unwrap();
    write_custom_host_config(&sandbox, &new);
    // The daemon reads the edit before the turn goes on.
    get(&client, ConfigNamespace::Agent).await;
    release.send(()).unwrap();
    let running = next_turn_end(&mut events, &session).await;
    sandbox.prompt(&client, &session).await.unwrap();
    let next = next_turn_end(&mut events, &session).await;

    // The running turn's request after the call went to the old host,
    // the only one that answers `old`; the session's title, asked for with
    // that turn, may still go there.
    assert_eq!(
        (answer(running), answer(next), new.models()),
        (
            "old".to_owned(),
            "new".to_owned(),
            vec!["mock-model".to_owned()]
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session whose model's custom provider was removed from the
// config fails its next turn with one configuration error that says so,
// rather than running on the provider it opened with (#1685).
// Owner: daemon next-turn providers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_provider_fails_the_next_turn_with_one_clear_error() {
    let host = ScriptedHost::spawn(&["mock-model"], |_| Reply::Text("answered"));
    let sandbox = Sandbox::new();
    let client = start_on_custom_host(&sandbox, &host).await;
    let session = sandbox.new_session(&client).await;
    let (_follower, mut events) = follow(&sandbox, &session).await;
    // The default model moves off it, so the config still loads.
    std::fs::write(
        &sandbox.isolated.config_path,
        format!("[model]\nprovider = \"xai\"\nmodel = \"grok-fixture\"\nauth = \"xai-oauth\"\n\n{CUSTOM_HOST_CONFIG_REST}"),
    )
    .unwrap();

    // The turn ends before it begins, which the prompt answers.
    let prompted = sandbox.prompt(&client, &session).await;

    assert_eq!(
        (
            prompted,
            next_turn_end(&mut events, &session).await,
            host.models()
        ),
        (
            Err(Some(ErrorCode::TurnEnded)),
            TurnOutcome::Failed {
                error: TurnError {
                    kind: TurnErrorKind::Configuration,
                    message: "invalid configuration: the session's model mock/mock-model is on provider 'mock', which is no longer configured; add it back, or switch the session to another model".into(),
                },
            },
            Vec::<String>::new(),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// The config after its `[model]` table in the tests whose session runs on
/// a custom host.
const CUSTOM_HOST_CONFIG_REST: &str = "[web_search]\nmode = \"off\"\n";

/// Writes a config whose model is `mock/mock-model` on `host`.
fn write_custom_host_config(sandbox: &Sandbox, host: &ScriptedHost) {
    std::fs::write(
        &sandbox.isolated.config_path,
        format!(
            "[model]\nprovider = \"mock\"\nmodel = \"mock-model\"\nauth = \"none\"\n\n{CUSTOM_HOST_CONFIG_REST}\n{}",
            host.provider_table("mock")
        ),
    )
    .unwrap();
}

/// Starts the daemon with the model on `host`, so new sessions run on it,
/// without the fixture provider, which would stand in for it.
async fn start_on_custom_host(sandbox: &Sandbox, host: &ScriptedHost) -> Client {
    write_custom_host_config(sandbox, host);
    let mut command = sandbox.rho("fixed");
    command.env_remove("RHO_AUTOMATION_TEST_MODE");
    sandbox.spawn_daemon(command, &[]).await.1
}

/// A following client attached to `session`, and its notifications.
async fn follow(sandbox: &Sandbox, session: &SessionId) -> (Client, Notifications) {
    let (follower, events) = sandbox.connect_following().await;
    follower
        .request::<SessionAttach>(&SessionAttachParams {
            session: session.clone(),
            after: None,
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        })
        .await
        .unwrap();
    (follower, events)
}

/// How `session`'s next turn ended.
async fn next_turn_end(notifications: &mut Notifications, session: &SessionId) -> TurnOutcome {
    loop {
        let DaemonNotification::SessionEvent(params) = next_notification(notifications).await
        else {
            continue;
        };
        assert_eq!(&params.session, session);
        if let SessionEvent::TurnEnded { outcome, .. } = params.event {
            return outcome;
        }
    }
}

/// The answer of a turn that ended `outcome`.
fn answer(outcome: TurnOutcome) -> String {
    match outcome {
        TurnOutcome::Answered { text, .. } => text,
        outcome => panic!("the turn did not answer: {outcome:?}"),
    }
}

// Covers: `codemode.mode`, which `/codemode` writes, reaches a session
// already loaded at its next turn: `only` stops offering the model the
// direct tools it had, keeps `codemode`, and tells the session's clients.
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_codemode_change_reaches_a_loaded_sessions_next_turn() {
    let sandbox = Sandbox::new();
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
    let (before, _) = next_turn(&mut events, &session).await;

    let state = get(&client, ConfigNamespace::Agent).await;
    set(
        &client,
        ConfigNamespace::Agent,
        &state.revision,
        vec![change("codemode.mode", SettingValue::Text("only".into()))],
    )
    .await
    .unwrap();
    sandbox.prompt(&client, &session).await.unwrap();
    let (after, notices) = next_turn(&mut events, &session).await;

    let offers = |tools: &[String], name: &str| tools.iter().any(|tool| tool == name);
    assert_eq!(
        (
            (offers(&before, "read_file"), offers(&before, "codemode")),
            (offers(&after, "read_file"), offers(&after, "codemode")),
            notices.len(),
        ),
        ((true, true), (false, true), 1),
        "before: {before:?}\nafter: {after:?}\nnotices: {notices:?}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a change against a stale revision is refused as a conflict, and
// one with a bad value or an unknown key as invalid params naming them, as
// is a credential store change, which must go through
// `rho/auth/choose_store`'s checks; none touches the file (#1720).
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
    let store = set(
        &client,
        ConfigNamespace::Agent,
        &revision,
        vec![change(
            "behavior.credential_store",
            SettingValue::Text("file".into()),
        )],
    )
    .await;
    assert_eq!(
        [stale.map(drop), refused.map(drop), store.map(drop)],
        [
            Err(RpcError::new(
                ErrorCode::ConfigConflict,
                "the settings changed since that revision; read them again",
            )),
            Err(RpcError::new(
                ErrorCode::InvalidParams,
                "`display.zen_mode`: no such setting\n`compaction.compact_threshold_percent`: must be at most 100; got 101",
            )),
            Err(RpcError::new(
                ErrorCode::InvalidParams,
                "`behavior.credential_store`: change it with rho/auth/choose_store",
            )),
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&sandbox.isolated.config_path).unwrap(),
        saved
    );
}

// Covers: a writer's changes sent before it hears back about its last one,
// all against the revision it last read, each land, since only its own
// changes followed that revision; another writer's change against it is
// refused as a conflict; a client's own namespace keeps the same rule
// (#1720).
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_quick_changes_do_not_conflict_with_each_other() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let read = get(&client, ConfigNamespace::Agent).await.revision;
    register_tui(&client).await;
    let tui_read = get(&client, tui()).await.revision;

    let mut sent = Vec::new();
    for (writer, key) in [
        ("tui-a", "behavior.rtk"),
        ("tui-a", "behavior.workspace_rewind"),
        ("tui-b", "behavior.inline_shell"),
    ] {
        let result = set_by(
            &client,
            Some(writer),
            ConfigNamespace::Agent,
            &read,
            vec![change(key, SettingValue::Bool(false))],
        )
        .await;
        sent.push(result.map(drop));
    }
    for zen in [true, false] {
        let result = set_by(
            &client,
            Some("tui-a"),
            tui(),
            &tui_read,
            vec![change("zen_mode", SettingValue::Bool(zen))],
        )
        .await;
        sent.push(result.map(drop));
    }
    let state = get(&client, ConfigNamespace::Agent).await;

    assert_eq!(
        (
            sent,
            value(&state, "behavior.rtk"),
            value(&state, "behavior.workspace_rewind"),
            value(&get(&client, tui()).await, "zen_mode"),
        ),
        (
            vec![
                Ok(()),
                Ok(()),
                Err(RpcError::new(
                    ErrorCode::ConfigConflict,
                    "the settings changed since that revision; read them again",
                )),
                Ok(()),
                Ok(()),
            ],
            global(SettingValue::Bool(false)),
            global(SettingValue::Bool(false)),
            global(SettingValue::Bool(false)),
        )
    );
}

// Covers: a hand edit of `config.toml` is served on the next request; one
// that no longer loads is reported in `errors` while the last good values
// stay served and in force, so a new session still opens; a change to the
// key it fails on repairs it (#1720).
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

    let hand_edit = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();
    set(
        &client,
        ConfigNamespace::Agent,
        &broken.revision,
        vec![change(
            "output.max_output_bytes",
            SettingValue::Integer(8192),
        )],
    )
    .await
    .unwrap();
    let repaired = get(&client, ConfigNamespace::Agent).await;
    assert_eq!(
        (
            value(&repaired, "output.max_output_bytes"),
            repaired.errors,
            std::fs::read_to_string(&sandbox.isolated.config_path).unwrap()
        ),
        (
            global(SettingValue::Integer(8192)),
            Vec::new(),
            hand_edit.replace("max_output_bytes = 0", "max_output_bytes = 8192")
        )
    );
}

// Covers: a change waits for no other writer of `config.toml`: while one
// holds the file's edit lock, as `rho login` does recording a login, the
// change is a conflict and writes nothing; a change to the value the file
// already holds leaves the file untouched (#1720).
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_skips_a_held_file_and_a_no_op_writes_nothing() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let path = &sandbox.isolated.config_path;
    let state = get(&client, ConfigNamespace::Agent).await;
    let lock_path = path.with_file_name(".config.toml.rho-edit.lock");
    let lock = std::fs::File::create(&lock_path).unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    let before = std::fs::read_to_string(path).unwrap();

    let held = set(
        &client,
        ConfigNamespace::Agent,
        &state.revision,
        vec![change("behavior.rtk", SettingValue::Bool(false))],
    )
    .await;
    assert_eq!(
        (
            held.map_err(|error| error.code),
            std::fs::read_to_string(path).unwrap()
        ),
        (Err(ErrorCode::ConfigConflict as i64), before)
    );
    drop(lock);

    let revision = set(
        &client,
        ConfigNamespace::Agent,
        &state.revision,
        vec![change("behavior.rtk", SettingValue::Bool(false))],
    )
    .await
    .unwrap();
    let written = std::fs::metadata(path).unwrap().modified().unwrap();
    let again = set(
        &client,
        ConfigNamespace::Agent,
        &revision,
        vec![change("behavior.rtk", SettingValue::Bool(false))],
    )
    .await
    .unwrap();
    assert_eq!(
        (again, std::fs::metadata(path).unwrap().modified().unwrap()),
        (revision, written)
    );
}

// Covers: a client's registered settings are stored for it and served
// again by a daemon started later, once the client registers again; a
// client file that stops reading keeps those values served, with the
// error, as `config.toml` does (#1720).
// Owner: daemon settings store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clients_settings_outlast_the_daemon() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    register_tui(&client).await;
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
    register_tui(&client).await;
    assert_eq!(
        value(&get(&client, tui()).await, "zen_mode"),
        global(SettingValue::Bool(true))
    );

    let path = sandbox
        .isolated
        .config_path
        .with_file_name("clients/tui.toml");
    std::fs::write(&path, "zen_mode = \n").unwrap();
    let broken = get(&client, tui()).await;
    assert_eq!(
        (
            value(&broken, "zen_mode"),
            broken.errors.len(),
            broken.errors[0].key.clone()
        ),
        (global(SettingValue::Bool(true)), 1, None)
    );
}
