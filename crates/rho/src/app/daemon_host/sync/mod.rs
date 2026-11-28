//! `rho/sync/*` and `rho/mcp/config/set`: saved API keys and MCP servers'
//! secret values copied from one daemon to another through a client that
//! sees only ciphertext, and reverted. [`seal`] owns the recipient keys and
//! the crypto, [`values`] what each item holds here, [`journal`] what a
//! sync replaced, [`keys`] and [`mcp`] writing and restoring each kind, and
//! [`revert`] status and revert over both; this module opens and seals
//! bundles and runs each request's store work off the async workers.
//!
//! Keys are read from the daemon's own credential store, the one its config
//! selects, with no client environment: a key held only in a variable is
//! not saved, so it is not exported. Only API keys are: interactive logins
//! hold refresh tokens the provider rotates, so a copy could sign both
//! machines out. MCP values are read from `config.toml`'s `[mcp.servers]`.
//!
//! The sealed plaintext is the JSON array of the values in `items` order,
//! and the `aad` is the JSON of `items`, so the list a bundle shows in the
//! clear is the list it holds. Nothing here logs a value.

mod journal;
mod keys;
mod mcp;
mod revert;
mod seal;
#[cfg(test)]
mod test_stores;
mod values;

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use rho_protocol::{
    ClientEnvironment, ErrorCode, ImportOutcome, RpcError, SealRecipient, SealSuite, SealedSecrets,
    SecretItem, SyncCompareParams, SyncCompareResponse, SyncDiscardParams, SyncDiscardResponse,
    SyncExportParams, SyncExportResponse, SyncImportKeysParams, SyncImportKeysResponse,
    SyncRecipientResponse,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    auth::{credentials_changed, off_workers, repository},
    sessions::{session_environment, Sessions},
    settings::ConfigStore,
};
use crate::app::credential_writes;

/// HPKE `info` for every bundle, which binds it to this use and version.
const INFO: &[u8] = b"rho sync secrets v1";

/// The one suite [`seal`] speaks.
const SUITE: SealSuite = SealSuite::X25519HkdfSha256ChaCha20Poly1305;

/// The daemon's sync state: the recipient keys it issued, and the lock
/// every journal read and write takes, so a revert's check and its write
/// see no import between them. Writers outside `rho/sync/*`, such as
/// `rho/auth/key`, do not take it.
#[derive(Default)]
pub(super) struct SecretSync {
    recipients: Arc<seal::Recipients>,
    journal: tokio::sync::Mutex<()>,
    stores: StoreSource,
}

/// Where [`SecretSync::offload`] finds its credential stores.
#[derive(Default)]
enum StoreSource {
    /// The daemon's own, the ones its config selects.
    #[default]
    Daemon,
    /// Fixed stores, for a test.
    #[cfg(test)]
    Fixed {
        keys: Arc<dyn rho_providers::credentials::CredentialStore>,
        journal: Arc<dyn rho_providers::credentials::CredentialStore>,
    },
}

impl SecretSync {
    /// Answers `rho/sync/recipient`, and drops the key once it expires even
    /// if no later sync request comes to sweep it. The timer holds the keys
    /// weakly, so none outlives this.
    pub(super) fn recipient(&self) -> SyncRecipientResponse {
        let public_key = self.recipients.issue(Instant::now());
        let recipients = Arc::downgrade(&self.recipients);
        tokio::spawn(async move {
            tokio::time::sleep(seal::RECIPIENT_LIFETIME).await;
            if let Some(recipients) = recipients.upgrade() {
                recipients.sweep(Instant::now());
            }
        });
        let expires_at = unix_now() + seal::RECIPIENT_LIFETIME.as_secs();
        SyncRecipientResponse::new(SealRecipient::new(SUITE, public_key, expires_at))
    }

    /// Answers `rho/sync/export`: reads the saved values off the async
    /// workers, then seals them.
    pub(super) async fn export(
        &self,
        params: SyncExportParams,
        settings: &ConfigStore,
    ) -> Result<SyncExportResponse, RpcError> {
        let SyncExportParams {
            recipient, items, ..
        } = params;
        known_suite(recipient.suite)?;
        let doc = settings.agent_document()?;
        let saved = self
            .offload(move |stores, _| values::saved_values(stores.keys, &doc, items))
            .await?;
        let sealed = seal_values(&recipient, saved.items, &saved.values)?;
        tracing::info!(
            sealed = sealed.items.len(),
            missing = saved.missing.len(),
            "sealed saved secrets for another daemon"
        );
        Ok(SyncExportResponse::new(sealed, saved.missing))
    }

    /// Answers `rho/sync/compare`: opens the bundle, then compares its
    /// values with the saved ones off the async workers.
    pub(super) async fn compare(
        &self,
        params: SyncCompareParams,
        settings: &ConfigStore,
    ) -> Result<SyncCompareResponse, RpcError> {
        let SyncCompareParams { sealed, .. } = params;
        let values = open_values(&self.recipients, Instant::now(), &sealed)?;
        let doc = settings.agent_document()?;
        let items = sealed.items;
        let compared = self
            .offload(move |stores, _| values::compare_values(stores.keys, &doc, items, &values))
            .await?;
        Ok(SyncCompareResponse::new(compared))
    }

    /// Answers `rho/sync/import_keys`: opens the bundle, saves its keys and
    /// journals what they replaced, then refreshes the sessions, whose
    /// models follow the store.
    pub(super) async fn import_keys(
        &self,
        params: SyncImportKeysParams,
        sessions: &Sessions,
    ) -> Result<SyncImportKeysResponse, RpcError> {
        let SyncImportKeysParams { sealed, sync, .. } = params;
        // Refused before opening, so the recipient stays usable: MCP values
        // go with `rho/mcp/config/set`.
        for item in &sealed.items {
            values::api_key_auth(item)?;
        }
        let values = open_values(&self.recipients, Instant::now(), &sealed)?;
        let items = sealed.items;
        let imported = self
            .with_journal(move |stores, now| keys::import(stores, &sync, items, &values, now))
            .await?;
        if imported
            .iter()
            .any(|imported| imported.outcome == ImportOutcome::Saved)
        {
            credentials_changed(sessions).await;
        }
        tracing::info!(items = imported.len(), "imported sealed API keys");
        Ok(SyncImportKeysResponse::new(imported))
    }

    /// Answers `rho/sync/discard`.
    pub(super) async fn discard(
        &self,
        params: SyncDiscardParams,
    ) -> Result<SyncDiscardResponse, RpcError> {
        let SyncDiscardParams { sync, .. } = params;
        self.with_journal(move |stores, now| keys::discard(stores, &sync, now))
            .await?;
        Ok(SyncDiscardResponse::default())
    }

    /// Deletes the journal if it expired, as the daemon starts.
    pub(super) async fn sweep_journal(&self) {
        let swept = self
            .with_journal(|stores, now| {
                journal::load(stores.journal, now)
                    .map(|_| ())
                    .map_err(|error| internal(format!("could not read the sync journal: {error}")))
            })
            .await;
        if let Err(error) = swept {
            tracing::warn!(error = %error.message, "could not sweep the sync journal");
        }
    }

    /// [`Self::offload`], holding the journal lock.
    async fn with_journal<T: Send + 'static>(
        &self,
        operation: impl FnOnce(keys::Stores, u64) -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        let _held = self.journal.lock().await;
        self.offload(operation).await
    }

    /// Runs `operation` on the daemon's own credential store, the one its
    /// config selects, off the async workers. Work on the journal holds its
    /// lock around this.
    async fn offload<T: Send + 'static>(
        &self,
        operation: impl FnOnce(keys::Stores, u64) -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        let (keys, journal, daemon) = match &self.stores {
            StoreSource::Daemon => {
                let environment = session_environment(&ClientEnvironment::default())?;
                (
                    environment.credentials(),
                    environment.untracked_credentials(),
                    true,
                )
            }
            #[cfg(test)]
            StoreSource::Fixed { keys, journal } => (Arc::clone(keys), Arc::clone(journal), false),
        };
        off_workers(move || {
            if daemon {
                credential_writes::load_config_and_store(&repository())?;
            }
            let stores = keys::Stores {
                keys: keys.as_ref(),
                journal: journal.as_ref(),
            };
            operation(stores, unix_now())
        })
        .await?
    }
}

/// The time now, in Unix seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Seals `values`, one per item of `items`, to `recipient`.
fn seal_values(
    recipient: &SealRecipient,
    items: Vec<SecretItem>,
    values: &[String],
) -> Result<SealedSecrets, RpcError> {
    // Sized up front so growing the buffer leaves no unzeroized copy: JSON
    // writes a byte as at most six (`\u0001`), plus quotes and a comma.
    let capacity = 2 + values
        .iter()
        .map(|value| 6 * value.len() + 3)
        .sum::<usize>();
    let mut plaintext = Zeroizing::new(Vec::with_capacity(capacity));
    serde_json::to_writer(&mut *plaintext, values).map_err(internal)?;
    let sealed = seal::seal(&recipient.public_key, INFO, &aad(&items)?, &plaintext)
        .map_err(|error| RpcError::new(ErrorCode::InvalidParams, error.to_string()))?;
    Ok(SealedSecrets::new(
        SUITE,
        recipient.public_key.clone(),
        items,
        sealed.encapped_key,
        sealed.ciphertext,
    ))
}

/// Opens `sealed` with a key `recipients` holds, using the key up, and
/// returns one value per item.
fn open_values(
    recipients: &seal::Recipients,
    now: Instant,
    sealed: &SealedSecrets,
) -> Result<Zeroizing<Vec<String>>, RpcError> {
    known_suite(sealed.suite)?;
    // Refused before opening, which would fail anyway: an unknown kind
    // reads back as `Unknown`, so the `aad` would not match.
    if sealed.items.contains(&SecretItem::Unknown) {
        return Err(invalid(
            "the bundle holds a kind of secret this daemon does not know",
        ));
    }
    refuse_duplicates(&sealed.items)?;
    let bundle = seal::Sealed {
        encapped_key: sealed.encapped_key.clone(),
        ciphertext: sealed.ciphertext.clone(),
    };
    let plaintext = recipients
        .open(now, &sealed.recipient, &bundle, INFO, &aad(&sealed.items)?)
        .map_err(|error| {
            let code = match error {
                seal::OpenError::Gone => ErrorCode::SealRecipientGone,
                seal::OpenError::Invalid(_) => ErrorCode::InvalidParams,
            };
            RpcError::new(code, error.to_string())
        })?;
    let values = Zeroizing::new(
        serde_json::from_slice::<Vec<String>>(&plaintext)
            .map_err(|_| invalid("the sealed values are not a JSON array of strings"))?,
    );
    if values.len() != sealed.items.len() {
        return Err(invalid("the bundle seals a value count unlike its items"));
    }
    Ok(values)
}

fn known_suite(suite: SealSuite) -> Result<(), RpcError> {
    if suite == SUITE {
        Ok(())
    } else {
        Err(invalid("this daemon does not know that seal suite"))
    }
}

fn aad(items: &[SecretItem]) -> Result<Vec<u8>, RpcError> {
    serde_json::to_vec(items).map_err(internal)
}

/// Refuses `items` that name a secret twice, which would leave which value
/// wins, and what a revert restores, to their order.
fn refuse_duplicates(items: &[SecretItem]) -> Result<(), RpcError> {
    let mut seen = HashSet::new();
    if items.iter().all(|item| seen.insert(item)) {
        Ok(())
    } else {
        Err(invalid("the items name a secret twice"))
    }
}

/// The SHA-256 of `value`, which compare and the journal match on.
fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn invalid(message: &str) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

fn internal(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(ErrorCode::InternalError, error.to_string())
}

fn store_error(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(
        ErrorCode::CredentialStoreUnavailable,
        format!("could not use the credential store: {error}"),
    )
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
