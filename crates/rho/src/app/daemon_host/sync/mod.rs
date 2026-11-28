//! `rho/sync/recipient`, `rho/sync/export`, and `rho/sync/compare`: saved
//! API keys copied from one daemon to another through a client that sees
//! only ciphertext. [`seal`] owns the keys and the crypto; this module
//! decides what is sealed, from where, and how it compares.
//!
//! Keys are read from the daemon's own credential store, the one its config
//! selects, with no client environment: a key held only in a variable is
//! not saved, so it is not exported. Only API keys are: interactive logins
//! hold refresh tokens the provider rotates, so a copy could sign both
//! machines out.
//!
//! The sealed plaintext is the JSON array of the values in `items` order,
//! and the `aad` is the JSON of `items`, so the list a bundle shows in the
//! clear is the list it holds. Nothing here logs a value.

mod seal;

use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use rho_protocol::{
    ClientEnvironment, CompareState, ErrorCode, RpcError, SealRecipient, SealSuite, SealedSecrets,
    SecretComparison, SecretItem, SyncCompareParams, SyncCompareResponse, SyncExportParams,
    SyncExportResponse, SyncRecipientResponse,
};
use rho_providers::credentials::CredentialStore;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::auth::{client_credentials, off_workers, repository};
use crate::app::credential_writes::{self, CredentialWriteError};

/// HPKE `info` for every bundle, which binds it to this use and version.
const INFO: &[u8] = b"rho sync secrets v1";

/// The one suite [`seal`] speaks.
const SUITE: SealSuite = SealSuite::X25519HkdfSha256ChaCha20Poly1305;

/// The daemon's sync state: the recipient keys it issued.
#[derive(Default)]
pub(super) struct SecretSync {
    recipients: Arc<seal::Recipients>,
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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let expires_at = (now + seal::RECIPIENT_LIFETIME).as_secs();
        SyncRecipientResponse::new(SealRecipient::new(SUITE, public_key, expires_at))
    }

    /// Answers `rho/sync/export`: reads the saved keys off the async
    /// workers, then seals them.
    pub(super) async fn export(
        &self,
        params: SyncExportParams,
    ) -> Result<SyncExportResponse, RpcError> {
        let SyncExportParams {
            recipient, items, ..
        } = params;
        known_suite(recipient.suite)?;
        let credentials = client_credentials(&ClientEnvironment::default())?;
        let saved = off_workers(move || {
            credential_writes::load_config_and_store(&repository())?;
            saved_values(credentials.as_ref(), items)
        })
        .await??;
        let sealed = seal_values(&recipient, saved.items, &saved.values)?;
        tracing::info!(
            sealed = sealed.items.len(),
            missing = saved.missing.len(),
            "sealed saved API keys for another daemon"
        );
        Ok(SyncExportResponse::new(sealed, saved.missing))
    }

    /// Answers `rho/sync/compare`: opens the bundle, then compares its
    /// values with the saved ones off the async workers.
    pub(super) async fn compare(
        &self,
        params: SyncCompareParams,
    ) -> Result<SyncCompareResponse, RpcError> {
        let SyncCompareParams { sealed, .. } = params;
        let values = open_values(&self.recipients, Instant::now(), &sealed)?;
        let credentials = client_credentials(&ClientEnvironment::default())?;
        let items = sealed.items;
        let compared = off_workers(move || {
            credential_writes::load_config_and_store(&repository())?;
            compare_values(credentials.as_ref(), items, &values)
        })
        .await??;
        Ok(SyncCompareResponse::new(compared))
    }
}

/// What [`saved_values`] found: the items that have a saved value, with
/// those values in the same order, and the items that have none.
struct Saved {
    items: Vec<SecretItem>,
    values: Zeroizing<Vec<String>>,
    missing: Vec<SecretItem>,
}

/// Reads each item's saved value from `store`, refusing an item this
/// daemon cannot export.
fn saved_values(store: &dyn CredentialStore, items: Vec<SecretItem>) -> Result<Saved, RpcError> {
    let mut saved = Saved {
        items: Vec::new(),
        values: Zeroizing::new(Vec::new()),
        missing: Vec::new(),
    };
    for item in items {
        match credential_writes::saved_api_key(api_key_auth(&item)?, store)? {
            Some(value) => {
                saved.items.push(item);
                saved.values.push(value);
            }
            None => saved.missing.push(item),
        }
    }
    Ok(saved)
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

/// How each of `values`, one per item of `items`, compares with the value
/// `store` holds for it.
fn compare_values(
    store: &dyn CredentialStore,
    items: Vec<SecretItem>,
    values: &[String],
) -> Result<Vec<SecretComparison>, RpcError> {
    items
        .into_iter()
        .zip(values)
        .map(|(item, value)| {
            // A profile the target does not know yet, such as a custom
            // provider only the source defines, holds nothing here.
            let held = match credential_writes::saved_api_key(api_key_auth(&item)?, store) {
                Err(CredentialWriteError::UnknownAuth { .. }) => None,
                held => held?.map(Zeroizing::new),
            };
            // Comparing digests keeps the time it takes from telling how
            // much of a guessed key is right.
            let state = match held {
                None => CompareState::Absent,
                Some(held) if digest(&held) == digest(value) => CompareState::Same,
                Some(_) => CompareState::Differs,
            };
            Ok(SecretComparison::new(item, state))
        })
        .collect()
}

/// The auth profile whose saved API key `item` names: the only kind this
/// daemon exports.
fn api_key_auth(item: &SecretItem) -> Result<&str, RpcError> {
    match item {
        SecretItem::ApiKey { auth, .. } => Ok(auth),
        // `Unknown`, or a kind a newer protocol adds.
        _ => Err(invalid("this daemon cannot export that kind of secret")),
    }
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

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn invalid(message: &str) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

fn internal(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(ErrorCode::InternalError, error.to_string())
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
