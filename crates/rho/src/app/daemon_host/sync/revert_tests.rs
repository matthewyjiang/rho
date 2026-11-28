use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_protocol::{
    McpConfigSetParams, McpServerChange, RevertedItem, SecretItem, SyncId, SyncItem,
};
use rho_providers::credentials::{CredentialStore, MemoryCredentialStore};
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::app::{
    credential_writes::api_key_account,
    daemon_host::sync::{keys::Stores, test_stores, test_stores::Refusing, unix_now},
};

const ANTHROPIC: &str = "anthropic-api-key";

// Covers: a revert whose journal update the store refuses still reports,
// and keeps, the key it restored, and says a key came back so the sessions
// read the keys again.
// Owner: sync revert (`sync/revert.rs`).
#[tokio::test]
async fn a_revert_reports_what_it_restored_even_if_the_journal_stays() {
    let home = TempDir::new().unwrap();
    let settings = test_stores::settings(home.path(), "");
    let store = Arc::new(MemoryCredentialStore::default());
    let account = api_key_account(ANTHROPIC).unwrap();
    store.set_secret(account, "sk-old").unwrap();
    let sync = SyncId::new("s");
    let stores = Stores {
        keys: store.as_ref(),
        journal: store.as_ref(),
    };
    // Two keys, so the journal outlives the revert of one and its update
    // is a write the store refuses.
    keys::import(
        stores,
        &sync,
        vec![
            SecretItem::api_key(ANTHROPIC),
            SecretItem::api_key("moonshot-api-key"),
        ],
        &["sk-new".to_owned(), "sk-m".to_owned()],
        unix_now(),
    )
    .unwrap();
    let refusing = Refusing {
        inner: store.clone(),
        account: journal::JOURNAL_ACCOUNT,
    };
    let secret_sync = SecretSync::with_stores(store.clone(), Arc::new(refusing));
    let item = SyncItem::api_key(ANTHROPIC);

    let reverted = secret_sync
        .revert(
            SyncRevertParams::new(sync.clone(), vec![item.clone()]),
            &settings,
        )
        .await
        .unwrap();
    assert_eq!(
        reverted.response.items,
        vec![RevertedItem::new(item.clone(), RevertOutcome::Restored)]
    );
    assert!(reverted.key_restored);
    assert_eq!(
        store.get_secret(account).unwrap().as_deref(),
        Some("sk-old")
    );
    let journal = keys::journal_of(stores, &sync, unix_now()).unwrap();
    assert!(journal.find(&item).is_some());
}

// Covers: a revert naming servers, keys, and items the sync did not change
// answers each in the order it named them.
// Owner: sync revert (`sync/revert.rs`).
#[tokio::test]
async fn a_revert_answers_each_item_in_order() {
    let home = TempDir::new().unwrap();
    let settings = test_stores::settings(home.path(), "");
    let store = Arc::new(MemoryCredentialStore::default());
    let secret_sync = SecretSync::with_stores(store.clone(), store.clone());
    let sync = SyncId::new("s");
    let stores = Stores {
        keys: store.as_ref(),
        journal: store.as_ref(),
    };
    keys::import(
        stores,
        &sync,
        vec![SecretItem::api_key(ANTHROPIC)],
        &["sk-new".to_owned()],
        unix_now(),
    )
    .unwrap();
    let put = McpServerChange::Put {
        identity: "plain".into(),
        value: json!({"transport": "stdio", "command": "plain"}),
        secrets: Vec::new(),
    };
    let revision = settings.mcp_config().unwrap().revision;
    secret_sync
        .set_mcp(
            McpConfigSetParams::new(revision, vec![put]).with_sync(sync.clone()),
            &settings,
        )
        .await
        .unwrap();
    let (server, untouched, key) = (
        SyncItem::mcp_server("plain"),
        SyncItem::api_key("moonshot-api-key"),
        SyncItem::api_key(ANTHROPIC),
    );

    let reverted = secret_sync
        .revert(
            SyncRevertParams::new(sync, vec![server.clone(), untouched.clone(), key.clone()]),
            &settings,
        )
        .await
        .unwrap();
    assert_eq!(
        reverted.response.items,
        vec![
            RevertedItem::new(server, RevertOutcome::Restored),
            RevertedItem::new(
                untouched,
                RevertOutcome::Failed {
                    message: "this sync did not change it, or it is already reverted".into()
                }
            ),
            RevertedItem::new(key, RevertOutcome::Restored),
        ]
    );
    assert_eq!(settings.mcp_config().unwrap().servers, vec![]);
}
