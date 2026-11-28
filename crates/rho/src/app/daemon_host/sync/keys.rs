//! Saving opened API keys on the target and restoring what they replaced:
//! what `rho/sync/import_keys`, `status`, `revert`, and `discard` do to a
//! credential store and its [`journal`].
//!
//! An import journals each key before it writes any, so a crash between
//! the two leaves a journal that can still restore everything; an item
//! whose write never happened reads as changed.

use rho_protocol::{
    ErrorCode, ImportOutcome, ImportedSecret, RevertOutcome, RevertedItem, RpcError, SecretItem,
    SyncId, SyncItem, SyncItemState, SyncItemStatus, SyncStatusResponse,
};
use rho_providers::credentials::{CredentialResult, CredentialStore};
use zeroize::Zeroizing;

use super::{
    api_key_auth,
    journal::{self, Journal, JournalItem, JOURNAL_LIFETIME},
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
    let loaded = journal::load(stores.journal, now).map_err(store_error)?;
    let mut journal = loaded
        .clone()
        .filter(|journal| &journal.sync == sync)
        .unwrap_or_else(|| Journal::new(sync.clone(), now));
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
                    let item = SyncItem::api_key(auth);
                    let displaced = journal.record(item.clone(), account, previous, value);
                    writes.push(Write {
                        index: outcomes.len(),
                        item,
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
    journal.expires_at = now + JOURNAL_LIFETIME;
    journal::save(stores.journal, &journal).map_err(store_error)?;
    let attempted = writes.len();
    let mut unwritten = 0;
    for write in writes {
        if let Err(error) = stores.keys.set_secret(write.account, write.value) {
            let message = credential_writes::redacted(error.to_string(), write.value);
            outcomes[write.index] = failed(format!("could not save the key: {message}"));
            journal.undo(&write.item, write.displaced);
            unwritten += 1;
        }
    }
    let settled = match (unwritten, loaded) {
        (0, _) => Ok(()),
        // Nothing written replaces nothing.
        (unwritten, Some(loaded)) if unwritten == attempted => {
            journal::save(stores.journal, &loaded)
        }
        (unwritten, None) if unwritten == attempted => journal::delete(stores.journal),
        _ => journal::save(stores.journal, &journal),
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

/// Whether each item `sync` changed still holds what it wrote.
pub(super) fn status(
    stores: Stores,
    sync: &SyncId,
    now: u64,
) -> Result<SyncStatusResponse, RpcError> {
    let journal = journal_of(stores, sync, now)?;
    let items = journal
        .items
        .iter()
        .map(|journaled| {
            let current = stores
                .keys
                .get_secret_fresh(&journaled.account)
                .map_err(store_error)?
                .map(Zeroizing::new);
            let state = if journaled.unchanged(current.as_deref().map(String::as_str)) {
                SyncItemState::Unchanged
            } else {
                SyncItemState::Changed
            };
            Ok(SyncItemStatus::new(journaled.item.clone(), state))
        })
        .collect::<Result<_, RpcError>>()?;
    Ok(SyncStatusResponse::new(journal.expires_at, items))
}

/// Restores each of `items` that `sync` changed and that still holds what
/// it wrote, or that `overwrite` names, and forgets the ones restored.
pub(super) fn revert(
    stores: Stores,
    sync: &SyncId,
    items: Vec<SyncItem>,
    overwrite: &[SyncItem],
    now: u64,
) -> Result<Vec<RevertedItem>, RpcError> {
    let mut journal = journal_of(stores, sync, now)?;
    let mut reverted = Vec::with_capacity(items.len());
    for item in items {
        let outcome = match journal.find(&item).cloned() {
            None => RevertOutcome::Failed {
                message: "this sync did not change it, or it is already reverted".into(),
            },
            Some(journaled) => match restore(stores.keys, &journaled, overwrite) {
                Ok(RevertOutcome::Restored) => {
                    journal.remove(&item);
                    RevertOutcome::Restored
                }
                Ok(outcome) => outcome,
                Err(error) => RevertOutcome::Failed {
                    message: format!("could not restore it: {error}"),
                },
            },
        };
        reverted.push(RevertedItem::new(item, outcome));
    }
    if reverted
        .iter()
        .any(|reverted| reverted.outcome == RevertOutcome::Restored)
    {
        if let Err(error) = journal::save(stores.journal, &journal) {
            // The keys are restored; their entries stay and read as changed,
            // and restoring one again with `overwrite` writes the same value.
            tracing::warn!(%error, "could not update the sync journal after a revert");
        }
    }
    Ok(reverted)
}

/// Deletes `sync`'s journal, leaving what it wrote.
pub(super) fn discard(stores: Stores, sync: &SyncId, now: u64) -> Result<(), RpcError> {
    journal_of(stores, sync, now)?;
    journal::delete(stores.journal).map_err(store_error)
}

/// Puts back what `journaled` replaced, unless something changed it since
/// the sync and `overwrite` does not name it.
fn restore(
    keys: &dyn CredentialStore,
    journaled: &JournalItem,
    overwrite: &[SyncItem],
) -> CredentialResult<RevertOutcome> {
    let current = keys
        .get_secret_fresh(&journaled.account)?
        .map(Zeroizing::new);
    if !journaled.unchanged(current.as_deref().map(String::as_str))
        && !overwrite.contains(&journaled.item)
    {
        return Ok(RevertOutcome::Changed);
    }
    match &journaled.previous {
        Some(previous) => keys.set_secret(&journaled.account, previous)?,
        None => {
            keys.delete_secret(&journaled.account)?;
        }
    }
    Ok(RevertOutcome::Restored)
}

/// The journal of `sync`, or `SyncNotFound` when the daemon keeps another
/// sync's or none.
fn journal_of(stores: Stores, sync: &SyncId, now: u64) -> Result<Journal, RpcError> {
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

fn store_error(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(
        ErrorCode::CredentialStoreUnavailable,
        format!("could not use the credential store: {error}"),
    )
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
