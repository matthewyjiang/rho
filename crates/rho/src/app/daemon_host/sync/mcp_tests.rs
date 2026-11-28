use std::{sync::Arc, time::Instant};

use pretty_assertions::assert_eq;
use rho_protocol::{
    ConfigRevision, McpSecret, McpSecretKind, McpSecretSlot, RevertedItem, SealRecipient, SyncId,
    SyncItem, SyncItemState, SyncItemStatus, SyncRevertParams, SyncStatusParams,
};
use rho_providers::credentials::MemoryCredentialStore;
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::app::daemon_host::sync::{seal_values, test_stores, SUITE};

const FILE: &str = r#"[mcp.servers.docs]
transport = "streamable_http"
url = "https://docs.example/mcp"
headers = { Authorization = "Bearer target" }
"#;

/// A target daemon's sync state and settings, its `config.toml` holding
/// [`FILE`].
struct Target {
    home: TempDir,
    settings: ConfigStore,
    sync: SecretSync,
}

impl Target {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let settings = test_stores::settings(home.path(), FILE);
        let store = Arc::new(MemoryCredentialStore::default());
        Self {
            home,
            settings,
            sync: SecretSync::with_stores(store.clone(), store),
        }
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.home.path().join("config.toml")).unwrap()
    }

    fn recipient(&self) -> SealRecipient {
        SealRecipient::new(SUITE, self.sync.recipients.issue(Instant::now()), 0)
    }

    /// Puts `docs` as [`FILE`] has it, its `Authorization` header from
    /// `sealed`, journaled under `sync` when given.
    async fn put_docs(
        &self,
        sealed: Option<SealedSecrets>,
        sync: Option<&SyncId>,
    ) -> Result<McpConfigSetResponse, RpcError> {
        let revision = self.settings.mcp_config()?.revision;
        self.put_docs_at(revision, sealed, sync).await
    }

    /// [`Self::put_docs`], made against `revision`.
    async fn put_docs_at(
        &self,
        revision: ConfigRevision,
        sealed: Option<SealedSecrets>,
        sync: Option<&SyncId>,
    ) -> Result<McpConfigSetResponse, RpcError> {
        let put = McpServerChange::Put {
            identity: "docs".into(),
            value: json!({"transport": "streamable_http", "url": "https://docs.example/mcp"}),
            secrets: vec![McpSecret::new(authorization(), SecretSource::Sealed)],
        };
        let mut params = McpConfigSetParams::new(revision, vec![put]);
        if let Some(sealed) = sealed {
            params = params.with_sealed(sealed);
        }
        if let Some(sync) = sync {
            params = params.with_sync(sync.clone());
        }
        self.sync.set_mcp(params, &self.settings).await
    }
}

fn authorization() -> McpSecretSlot {
    McpSecretSlot::new(McpSecretKind::Header, "Authorization")
}

fn seal(recipient: &SealRecipient, item: SecretItem, value: &str) -> SealedSecrets {
    seal_values(recipient, vec![item], &[value.to_owned()]).unwrap()
}

fn error_code<T>(result: Result<T, RpcError>) -> Option<rho_protocol::ErrorCode> {
    result.err().and_then(|error| error.error_code())
}

// Covers: a set whose bundle is not exactly the secrets its puts take from
// it, that takes one with no bundle, or that was made against a stale
// revision, is refused before opening and writes nothing, so the same
// recipient still opens the right bundle.
// Owner: sync MCP writes (`sync/mcp.rs`).
#[tokio::test]
async fn a_bundle_the_puts_do_not_ask_for_is_refused_before_opening() {
    let target = Target::new();
    let recipient = target.recipient();
    let other = seal(
        &recipient,
        SecretItem::mcp_header("docs", "X-Other"),
        "Bearer source",
    );

    for refused in [Some(other), None] {
        assert_eq!(
            error_code(target.put_docs(refused, None).await),
            Some(rho_protocol::ErrorCode::InvalidParams)
        );
    }
    let asked = seal(
        &recipient,
        SecretItem::mcp_header("docs", "Authorization"),
        "Bearer source",
    );
    assert_eq!(
        error_code(
            target
                .put_docs_at(ConfigRevision::new("stale"), Some(asked.clone()), None)
                .await
        ),
        Some(rho_protocol::ErrorCode::ConfigConflict)
    );
    assert_eq!(target.file(), FILE);
    target.put_docs(Some(asked), None).await.unwrap();
    assert_eq!(
        target.file(),
        FILE.replace("Bearer target", "Bearer source")
    );
}

// Covers: a server changed on the target since a journaled set reads as
// changed, and a revert leaves it until told to overwrite, then puts back
// the entry from before the set, secret value included.
// Owner: sync MCP writes and revert (`sync/mcp.rs`, `sync/revert.rs`).
#[tokio::test]
async fn a_revert_leaves_a_server_changed_since_the_sync_unless_overwritten() {
    let target = Target::new();
    let sync = SyncId::new("press-1");
    let sealed = seal(
        &target.recipient(),
        SecretItem::mcp_header("docs", "Authorization"),
        "Bearer source",
    );
    target.put_docs(Some(sealed), Some(&sync)).await.unwrap();
    let mine = FILE.replace("Bearer target", "Bearer mine");
    std::fs::write(target.home.path().join("config.toml"), &mine).unwrap();
    let docs = SyncItem::mcp_server("docs");

    assert_eq!(
        target
            .sync
            .status(SyncStatusParams::new(sync.clone()), &target.settings)
            .await
            .unwrap()
            .items,
        vec![SyncItemStatus::new(docs.clone(), SyncItemState::Changed)]
    );
    let revert = SyncRevertParams::new(sync, vec![docs.clone()]);
    let reverted = target
        .sync
        .revert(revert.clone(), &target.settings)
        .await
        .unwrap();
    assert_eq!(
        reverted.response.items,
        vec![RevertedItem::new(docs.clone(), RevertOutcome::Changed)]
    );
    assert_eq!(target.file(), mine);
    let reverted = target
        .sync
        .revert(revert.with_overwrite(vec![docs.clone()]), &target.settings)
        .await
        .unwrap();
    assert_eq!(
        reverted.response.items,
        vec![RevertedItem::new(docs, RevertOutcome::Restored)]
    );
    assert!(!reverted.key_restored);
    assert_eq!(target.file(), FILE);
}
