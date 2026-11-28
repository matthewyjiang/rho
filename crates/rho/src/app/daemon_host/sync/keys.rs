//! Saving opened API keys on the target and restoring what they replaced:
//! what `rho/sync/import_keys` and a revert of an API key do to a
//! credential store and its [`journal`].
//!
//! An import journals each key before it writes any, so a crash between
//! the two leaves a journal that can still restore everything; an item
//! whose write never happened reads as changed.

use rho_protocol::{
    ErrorCode, ImportOutcome, ImportedSecret, RevertOutcome, RpcError, SecretItem, SyncId, SyncItem,
};
use rho_providers::credentials::{CredentialResult, CredentialStore};
use zeroize::Zeroizing;

use super::{
    journal::{self, Draft, Journal, JournalItem, Place},
    store_error,
    values::api_key_auth,
};
use crate::app::credential_writes::{self, CredentialWriteError};

/// The daemon's credential store, twice: `keys` for the API keys, whose
/// writes rebuild the sessions' providers, and `journal` for the journal,
/// whose writes must not, since no provider reads it.
#[derive(Clone, Copy)]
pub(super) struct Stores<'a> {
    pub(super) keys: &'a dyn CredentialStore,
    pub(super) journal: &'a dyn CredentialStore,
}

/// Saves `values`, one per item of `items`, which name each item once,
/// journaling under `sync` what each replaced. The journal of another sync
/// is replaced once a key is written; an import that writes none leaves
/// the journal as it was. A store that cannot be read fails the request
/// before anything is written.
///
/// Keys are saved as sent, untrimmed, so comparing the same bundle later
/// reads `same`. Only the key is written: an Ollama login record, like
/// every provider profile, is the client's to copy, and to revert.
pub(super) fn import(
    stores: Stores,
    sync: &SyncId,
    items: Vec<SecretItem>,
    values: &[String],
    now: u64,
) -> Result<Vec<ImportedSecret>, RpcError> {
    let mut draft = Draft::begin(stores.journal, sync, now).map_err(store_error)?;
    let mut outcomes = Vec::with_capacity(items.len());
    let mut writes = Vec::new();
    for (item, value) in items.iter().zip(values) {
        let auth = api_key_auth(item)?;
        let outcome = if value.trim().is_empty() {
            failed(CredentialWriteError::BlankKey)
        } else {
            match credential_writes::api_key_account(auth) {
                Ok(account) => {
                    let previous = stores.keys.get_secret_fresh(account).map_err(store_error)?;
                    let place = Place::ApiKey {
                        auth: auth.to_owned(),
                        account: account.to_owned(),
                    };
                    let displaced = draft.journal.record(place, previous, Some(value));
                    writes.push(Write {
                        index: outcomes.len(),
                        item: SyncItem::api_key(auth),
                        account,
                        value,
                        displaced,
                    });
                    ImportOutcome::Saved
                }
                Err(error) => failed(error),
            }
        };
        outcomes.push(outcome);
    }
    if writes.is_empty() {
        return Ok(imported(items, outcomes));
    }
    draft.save(stores.journal).map_err(store_error)?;
    let attempted = writes.len();
    let mut unwritten = 0;
    for write in writes {
        if let Err(error) = stores.keys.set_secret(write.account, write.value) {
            let message = credential_writes::redacted(error.to_string(), write.value);
            outcomes[write.index] = failed(format!("could not save the key: {message}"));
            draft.journal.undo(&write.item, write.displaced);
            unwritten += 1;
        }
    }
    let settled = match unwritten {
        0 => Ok(()),
        // Nothing written replaces nothing.
        unwritten if unwritten == attempted => draft.abandon(stores.journal),
        _ => draft.save(stores.journal),
    };
    if let Err(error) = settled {
        // The journal still names the keys that were not written; they read
        // as changed, so a revert leaves them unless told to overwrite.
        tracing::warn!(%error, "could not update the sync journal after a failed key write");
    }
    Ok(imported(items, outcomes))
}

/// A key [`import`] journaled and is about to write.
struct Write<'a> {
    /// Its place in the outcomes.
    index: usize,
    item: SyncItem,
    account: &'static str,
    value: &'a str,
    /// The journal entry its record replaced, put back if the write fails.
    displaced: Option<JournalItem>,
}

/// The value the store account `account` holds now.
pub(super) fn current(
    keys: &dyn CredentialStore,
    account: &str,
) -> CredentialResult<Option<Zeroizing<String>>> {
    Ok(keys.get_secret_fresh(account)?.map(Zeroizing::new))
}

/// Puts back the key `journaled` replaced at `account`, unless something
/// changed it since the sync and `overwrite` is false.
pub(super) fn restore(
    keys: &dyn CredentialStore,
    account: &str,
    journaled: &JournalItem,
    overwrite: bool,
) -> CredentialResult<RevertOutcome> {
    let current = current(keys, account)?;
    if !journaled.unchanged(current.as_deref().map(String::as_str)) && !overwrite {
        return Ok(RevertOutcome::Changed);
    }
    match &journaled.previous {
        Some(previous) => keys.set_secret(account, previous)?,
        None => {
            keys.delete_secret(account)?;
        }
    }
    Ok(RevertOutcome::Restored)
}

/// The journal of `sync`, or `SyncNotFound` when the daemon keeps another
/// sync's or none.
pub(super) fn journal_of(stores: Stores, sync: &SyncId, now: u64) -> Result<Journal, RpcError> {
    journal::load(stores.journal, now)
        .map_err(store_error)?
        .filter(|journal| &journal.sync == sync)
        .ok_or_else(|| {
            RpcError::new(
                ErrorCode::SyncNotFound,
                format!(
                    "this daemon keeps no journal for sync {sync}: a later sync replaced it, it expired, or it was reverted or discarded"
                ),
            )
        })
}

/// Deletes `sync`'s journal, leaving what it wrote.
pub(super) fn discard(stores: Stores, sync: &SyncId, now: u64) -> Result<(), RpcError> {
    journal_of(stores, sync, now)?;
    journal::delete(stores.journal).map_err(store_error)
}

fn imported(items: Vec<SecretItem>, outcomes: Vec<ImportOutcome>) -> Vec<ImportedSecret> {
    items
        .into_iter()
        .zip(outcomes)
        .map(|(item, outcome)| ImportedSecret::new(item, outcome))
        .collect()
}

fn failed(message: impl ToString) -> ImportOutcome {
    ImportOutcome::Failed {
        message: message.to_string(),
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
