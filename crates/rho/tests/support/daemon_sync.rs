//! Copying a saved API key from one daemon to another sealed
//! (`rho/sync/*`): the client relays only ciphertext.

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{SyncCompare, SyncExport, SyncRecipient},
    CompareState, ErrorCode, SecretComparison, SecretItem, SyncCompareParams, SyncCompareResponse,
    SyncExportParams, SyncRecipientParams,
};
use rho_providers::credentials::CredentialStore;

use super::auth::{rpc_error, Fixture, FIRST_KEY, KEY_ACCOUNT};

fn api_key(auth: &str) -> SecretItem {
    SecretItem::ApiKey { auth: auth.into() }
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
