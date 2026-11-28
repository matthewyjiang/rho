//! The structured tables of `config.toml` read and written entry by entry
//! through the daemon's socket (`rho/config/table/*`), as a client copying
//! rows of them from another daemon does.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{ConfigGet, ConfigSet, ConfigTableGet, ConfigTableSet},
    Client, ClientError, ConfigChangedParams, ConfigGetParams, ConfigLayer, ConfigNamespace,
    ConfigRevision, ConfigSetParams, ConfigTable, ConfigTableGetParams, ConfigTableGetResponse,
    ConfigTableSetParams, DaemonNotification, ErrorCode, ModelSelection, SettingChange,
    SettingEntry, SettingKey, SettingValue, TableChange, TableEntry, ValueSource,
};
use serde_json::json;

use super::sessions::{Sandbox, DEADLINE};

const CONFIG: &str = r#"[model]
provider = "xai"
model = "@fast"
auth = "xai-oauth"

[model.aliases]
# The alias the conversation model names.
fast = "xai/grok-fixture"

[web_search]
mode = "off"
"#;

async fn get(client: &Client, table: ConfigTable) -> ConfigTableGetResponse {
    client
        .request::<ConfigTableGet>(&ConfigTableGetParams::new(table))
        .await
        .unwrap()
}

/// Sends `changes` to `table` against `revision`: the new revision, or the
/// code of the error that refused them.
async fn set(
    client: &Client,
    table: ConfigTable,
    revision: &ConfigRevision,
    changes: Vec<TableChange>,
) -> Result<ConfigRevision, ErrorCode> {
    client
        .request::<ConfigTableSet>(&ConfigTableSetParams::new(table, revision.clone(), changes))
        .await
        .map(|response| response.revision)
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code().unwrap(),
            error => panic!("set failed outside the protocol: {error}"),
        })
}

fn put(key: &str, value: serde_json::Value) -> TableChange {
    TableChange::Put {
        key: key.into(),
        value,
    }
}

// Covers: a client puts table entries only at the table's revision and only
// as the loader takes them, keeping the file's comments; reading the table
// back gives what it wrote, and a write that moves a setting (the alias the
// conversation model names) tells every client reading the settings, under
// the settings revision read before it, which a settings write may still
// send, as a sync copying aliases and then settings does.
// Owner: daemon settings store.
#[tokio::test]
async fn a_table_write_lands_at_its_revision_and_moves_the_settings_under_theirs() {
    let sandbox = Sandbox::new();
    std::fs::write(&sandbox.isolated.config_path, CONFIG).unwrap();
    let client = sandbox.start_daemon("fixed").await;
    let (reader, mut changes) = sandbox.connect_following().await;
    let settings = reader
        .request::<ConfigGet>(&ConfigGetParams {
            namespace: ConfigNamespace::Agent,
        })
        .await
        .unwrap();

    let aliases = get(&client, ConfigTable::ModelAliases).await;
    assert_eq!(
        aliases.entries,
        [TableEntry::new("fast".into(), json!("xai/grok-fixture"))]
    );
    let written = set(
        &client,
        ConfigTable::ModelAliases,
        &aliases.revision,
        vec![
            put("fast", json!("openai/gpt-5.5")),
            put("grok.v2", json!("xai/grok-fixture")),
        ],
    )
    .await
    .unwrap();
    let read = get(&client, ConfigTable::ModelAliases).await;
    assert_eq!(
        (read.revision, read.entries),
        (
            written,
            vec![
                TableEntry::new("fast".into(), json!("openai/gpt-5.5")),
                TableEntry::new("grok.v2".into(), json!("xai/grok-fixture")),
            ]
        )
    );
    let notified = tokio::time::timeout(DEADLINE, changes.recv())
        .await
        .expect("the daemon notifies")
        .expect("the connection stays open");
    let DaemonNotification::ConfigChanged(ConfigChangedParams {
        revision, values, ..
    }) = notified
    else {
        panic!("expected rho/config/changed, got {notified:?}");
    };
    let settings_written = client
        .request::<ConfigSet>(&ConfigSetParams {
            namespace: ConfigNamespace::Agent,
            layer: ConfigLayer::Global,
            revision: settings.revision.clone(),
            writer: None,
            changes: vec![SettingChange::Set {
                key: SettingKey::new("web_search.mode"),
                value: SettingValue::Text("auto".into()),
            }],
        })
        .await
        .map(drop)
        .map_err(|error| error.to_string());
    assert_eq!(
        (
            revision,
            values.get(&SettingKey::new("model")),
            settings_written
        ),
        (
            settings.revision,
            Some(&SettingEntry {
                value: Some(SettingValue::Model(ModelSelection::Rho {
                    provider: "openai".into(),
                    model: "@fast".into(),
                })),
                source: ValueSource::Global,
            }),
            Ok(()),
        )
    );

    let saved = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();
    let providers = get(&client, ConfigTable::Providers).await;
    let refused = [
        set(
            &client,
            ConfigTable::ModelAliases,
            &aliases.revision,
            vec![put("fast", json!("xai/grok-fixture"))],
        )
        .await,
        set(
            &client,
            ConfigTable::Providers,
            &providers.revision,
            vec![put(
                "local",
                json!({"base_url": "http://127.0.0.1:8080/v1", "colour": "red"}),
            )],
        )
        .await,
    ];
    assert_eq!(
        refused,
        [
            Err(ErrorCode::ConfigConflict),
            Err(ErrorCode::InvalidParams)
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&sandbox.isolated.config_path).unwrap(),
        saved
    );
    assert!(
        saved.contains("# The alias the conversation model names.\nfast = \"openai/gpt-5.5\""),
        "{saved}"
    );
}
