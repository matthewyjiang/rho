//! Copying a saved API key, or an MCP server with its secret values, from
//! one daemon to another sealed (`rho/sync/*`, `rho/mcp/config/*`): the
//! client relays only ciphertext.

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{
        McpConfigGet, McpConfigSet, SyncCompare, SyncExport, SyncImportKeys, SyncRecipient,
        SyncRevert, SyncStatus,
    },
    Client, CompareState, ErrorCode, ImportOutcome, ImportedSecret, McpConfigGetParams,
    McpConfigSetParams, McpSecret, McpSecretKind, McpSecretSlot, McpServerChange, RevertOutcome,
    RevertedItem, SealedSecrets, SecretComparison, SecretItem, SecretSource, SyncCompareParams,
    SyncCompareResponse, SyncExportParams, SyncId, SyncImportKeysParams, SyncImportKeysResponse,
    SyncItem, SyncItemState, SyncItemStatus, SyncRecipientParams, SyncRevertParams,
    SyncRevertResponse, SyncStatusParams,
};
use rho_providers::credentials::CredentialStore;

use super::auth::{rpc_error, Fixture, FIRST_KEY, KEY_ACCOUNT};

fn api_key(auth: &str) -> SecretItem {
    SecretItem::api_key(auth)
}

/// The source's saved `mock` key, sealed to a recipient the target issued.
async fn sealed_mock_key(from: &Client, to: &Client) -> SealedSecrets {
    let recipient = to
        .request::<SyncRecipient>(&SyncRecipientParams::default())
        .await
        .unwrap()
        .recipient;
    from.request::<SyncExport>(&SyncExportParams::new(
        recipient,
        vec![api_key("mock-api-key")],
    ))
    .await
    .unwrap()
    .sealed
}

// Covers: a key saved in one daemon's store, for a provider its config
// defines, reaches another daemon sealed to that daemon's recipient; the
// target opens it once, comparing it with its own, and the used recipient
// opens nothing more. A key the source has not saved is reported missing.
// Owner: daemon e2e (two daemons over their sockets).
#[tokio::test]
async fn a_saved_key_travels_sealed_between_daemons() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let source = Fixture::start_with(&valid, |_| {}).await;
    let target = Fixture::start_with(&valid, |_| {}).await;
    source.store.set_secret(KEY_ACCOUNT, "sk-source").unwrap();
    target.store.set_secret(KEY_ACCOUNT, "sk-target").unwrap();
    let from = source.sandbox.connect().await;
    let to = target.sandbox.connect().await;

    let recipient = to
        .request::<SyncRecipient>(&SyncRecipientParams::default())
        .await
        .unwrap()
        .recipient;
    let exported = from
        .request::<SyncExport>(&SyncExportParams::new(
            recipient,
            vec![api_key("mock-api-key"), api_key("anthropic-api-key")],
        ))
        .await
        .unwrap();
    assert_eq!(exported.missing, vec![api_key("anthropic-api-key")]);

    let compare = SyncCompareParams::new(exported.sealed);
    assert_eq!(
        to.request::<SyncCompare>(&compare).await.unwrap(),
        SyncCompareResponse::new(vec![SecretComparison::new(
            api_key("mock-api-key"),
            CompareState::Differs
        )])
    );
    assert_eq!(
        to.request::<SyncCompare>(&compare)
            .await
            .map_err(rpc_error)
            .unwrap_err()
            .error_code(),
        Some(ErrorCode::SealRecipientGone)
    );
}

// Covers: a sealed key imported on the target replaces its own and is
// journaled under the sync; once the target's key changes again, a revert
// leaves it until told to overwrite, then restores the target's original
// key and forgets the sync.
// Owner: daemon e2e (two daemons over their sockets).
#[tokio::test]
async fn an_imported_key_reverts_to_the_targets_own() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let source = Fixture::start_with(&valid, |_| {}).await;
    let target = Fixture::start_with(&valid, |_| {}).await;
    source.store.set_secret(KEY_ACCOUNT, "sk-source").unwrap();
    target.store.set_secret(KEY_ACCOUNT, "sk-target").unwrap();
    let from = source.sandbox.connect().await;
    let to = target.sandbox.connect().await;
    let sync = SyncId::new("press-1");
    let item = SyncItem::api_key("mock-api-key");

    let sealed = sealed_mock_key(&from, &to).await;
    assert_eq!(
        to.request::<SyncImportKeys>(&SyncImportKeysParams::new(sealed, sync.clone()))
            .await
            .unwrap(),
        SyncImportKeysResponse::new(vec![ImportedSecret::new(
            api_key("mock-api-key"),
            ImportOutcome::Saved
        )])
    );
    assert_eq!(target.saved_key().as_deref(), Some("sk-source"));

    target.store.set_secret(KEY_ACCOUNT, "sk-changed").unwrap();
    assert_eq!(
        to.request::<SyncStatus>(&SyncStatusParams::new(sync.clone()))
            .await
            .unwrap()
            .items,
        vec![SyncItemStatus::new(item.clone(), SyncItemState::Changed)]
    );
    let revert = SyncRevertParams::new(sync.clone(), vec![item.clone()]);
    assert_eq!(
        to.request::<SyncRevert>(&revert).await.unwrap(),
        SyncRevertResponse::new(vec![RevertedItem::new(
            item.clone(),
            RevertOutcome::Changed
        )])
    );
    assert_eq!(
        to.request::<SyncRevert>(&revert.with_overwrite(vec![item.clone()]))
            .await
            .unwrap(),
        SyncRevertResponse::new(vec![RevertedItem::new(item, RevertOutcome::Restored)])
    );
    assert_eq!(target.saved_key().as_deref(), Some("sk-target"));
    assert_eq!(
        to.request::<SyncStatus>(&SyncStatusParams::new(sync))
            .await
            .map_err(rpc_error)
            .unwrap_err()
            .error_code(),
        Some(ErrorCode::SyncNotFound)
    );
}

// Covers: an MCP server's header travels sealed: the source reads the
// server without the value and seals it to the target, which writes the
// server with it under a sync, and a revert removes the server again.
// Owner: daemon e2e (two daemons over their sockets).
#[tokio::test]
async fn an_mcp_server_travels_with_its_header_sealed_and_reverts() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let source = Fixture::start_with(&valid, |_| {}).await;
    let target = Fixture::start_with(&valid, |_| {}).await;
    let source_file = &source.sandbox.isolated.config_path;
    let mut config = std::fs::read_to_string(source_file).unwrap();
    config.push_str(
        "\n[mcp.servers.docs]\ntransport = \"streamable_http\"\nurl = \"https://docs.example/mcp\"\nheaders = { Authorization = \"Bearer source\" }\n",
    );
    std::fs::write(source_file, config).unwrap();
    let from = source.sandbox.connect().await;
    let to = target.sandbox.connect().await;
    let sync = SyncId::new("press-1");
    let authorization = McpSecretSlot::new(McpSecretKind::Header, "Authorization");

    let read = from
        .request::<McpConfigGet>(&McpConfigGetParams::default())
        .await
        .unwrap();
    assert!(!serde_json::to_string(&read)
        .unwrap()
        .contains("Bearer source"));
    let [docs] = &read.servers[..] else {
        panic!("{read:?}");
    };
    assert_eq!(docs.secrets, vec![authorization.clone()]);
    let recipient = to
        .request::<SyncRecipient>(&SyncRecipientParams::default())
        .await
        .unwrap()
        .recipient;
    let exported = from
        .request::<SyncExport>(&SyncExportParams::new(
            recipient,
            vec![SecretItem::mcp_header("docs", "Authorization")],
        ))
        .await
        .unwrap();
    assert_eq!(exported.missing, vec![]);

    let revision = to
        .request::<McpConfigGet>(&McpConfigGetParams::default())
        .await
        .unwrap()
        .revision;
    let put = McpServerChange::Put {
        identity: "docs".into(),
        value: docs.value.clone(),
        secrets: vec![McpSecret::new(authorization, SecretSource::Sealed)],
    };
    to.request::<McpConfigSet>(
        &McpConfigSetParams::new(revision, vec![put])
            .with_sealed(exported.sealed)
            .with_sync(sync.clone()),
    )
    .await
    .unwrap();
    let written: toml::Table =
        toml::from_str(&std::fs::read_to_string(&target.sandbox.isolated.config_path).unwrap())
            .unwrap();
    assert_eq!(
        written["mcp"]["servers"]["docs"]["headers"]["Authorization"].as_str(),
        Some("Bearer source")
    );

    let item = SyncItem::mcp_server("docs");
    assert_eq!(
        to.request::<SyncStatus>(&SyncStatusParams::new(sync.clone()))
            .await
            .unwrap()
            .items,
        vec![SyncItemStatus::new(item.clone(), SyncItemState::Unchanged)]
    );
    assert_eq!(
        to.request::<SyncRevert>(&SyncRevertParams::new(sync, vec![item.clone()]))
            .await
            .unwrap(),
        SyncRevertResponse::new(vec![RevertedItem::new(item, RevertOutcome::Restored)])
    );
    assert_eq!(
        to.request::<McpConfigGet>(&McpConfigGetParams::default())
            .await
            .unwrap()
            .servers,
        vec![]
    );
}
