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
use rho_providers::credentials::CredentialStore;
use zeroize::Zeroizing;

use super::{
    api_key_auth,
    journal::{self, Journal, JOURNAL_LIFETIME},
};
use crate::app::credential_writes;

/// Saves `values`, one per item of `items`, to `store`, journaling under
/// `sync` what each replaced. `environment` reads the daemon's own
/// variables: a profile whose credential one of them holds is not saved,
/// since the variable wins over a saved key.
///
/// Only the key is written. An Ollama login record, like every provider
/// profile, is the client's to copy, and to revert.
pub(super) fn import(
    store: &dyn CredentialStore,
    sync: &SyncId,
    items: Vec<SecretItem>,
    values: &[String],
    now: u64,
    environment: impl Fn(&str) -> Option<String>,
) -> Result<Vec<ImportedSecret>, RpcError> {
    let mut journal = journal::load(store, now)
        .map_err(store_error)?
        .filter(|journal| &journal.sync == sync)
        .unwrap_or_else(|| Journal::new(sync.clone(), now));
    let before = journal.clone();
    let mut outcomes = Vec::with_capacity(items.len());
    let mut writes = Vec::new();
    for (item, value) in items.iter().zip(values) {
        let auth = api_key_auth(item)?;
        let outcome =
            if let Some(variable) = credential_writes::environment_override(auth, &environment) {
                ImportOutcome::Overridden {
                    variable: variable.to_owned(),
                }
            } else if value.trim().is_empty() {
                failed("the key is blank")
            } else {
                match credential_writes::api_key_account(auth) {
                    Ok(account) => {
                        let previous = store.get_secret_fresh(account).map_err(store_error)?;
                        journal.record(SyncItem::api_key(auth), account, previous, value);
                        writes.push((outcomes.len(), auth, account, value));
                        ImportOutcome::Saved
                    }
                    Err(error) => failed(&error.to_string()),
                }
            };
        outcomes.push(outcome);
    }
    if writes.is_empty() {
        return Ok(imported(items, outcomes));
    }
    journal.expires_at = now + JOURNAL_LIFETIME;
    journal::save(store, &journal).map_err(store_error)?;
    let mut unwritten = false;
    for (index, auth, account, value) in writes {
        if let Err(error) = store.set_secret(account, value) {
            outcomes[index] = failed(&format!("could not save the key: {error}"));
            // Back to what the journal held for it before this import.
            let item = SyncItem::api_key(auth);
            journal.remove(&item);
            journal.items.extend(before.find(&item).cloned());
            unwritten = true;
        }
    }
    if unwritten {
        journal::save(store, &journal).map_err(store_error)?;
    }
    Ok(imported(items, outcomes))
}

/// Whether each item `sync` changed still holds what it wrote.
pub(super) fn status(
    store: &dyn CredentialStore,
    sync: &SyncId,
    now: u64,
) -> Result<SyncStatusResponse, RpcError> {
    let journal = journal_of(store, sync, now)?;
    let items = journal
        .items
        .iter()
        .map(|journaled| {
            let current = store
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
    store: &dyn CredentialStore,
    sync: &SyncId,
    items: Vec<SyncItem>,
    overwrite: &[SyncItem],
    now: u64,
) -> Result<Vec<RevertedItem>, RpcError> {
    let mut journal = journal_of(store, sync, now)?;
    let mut reverted = Vec::with_capacity(items.len());
    for item in items {
        let outcome = match journal.find(&item).cloned() {
            None => RevertOutcome::Failed {
                message: "this sync did not change it, or it is already reverted".into(),
            },
            Some(journaled) => match restore(store, &journaled, overwrite) {
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
    journal::save(store, &journal).map_err(store_error)?;
    Ok(reverted)
}

/// Deletes `sync`'s journal, leaving what it wrote.
pub(super) fn discard(
    store: &dyn CredentialStore,
    sync: &SyncId,
    now: u64,
) -> Result<(), RpcError> {
    journal_of(store, sync, now)?;
    journal::delete(store).map_err(store_error)
}

/// Puts back what `journaled` replaced, unless something changed it since
/// the sync and `overwrite` does not name it.
fn restore(
    store: &dyn CredentialStore,
    journaled: &journal::JournalItem,
    overwrite: &[SyncItem],
) -> rho_providers::credentials::CredentialResult<RevertOutcome> {
    let current = store
        .get_secret_fresh(&journaled.account)?
        .map(Zeroizing::new);
    if !journaled.unchanged(current.as_deref().map(String::as_str))
        && !overwrite.contains(&journaled.item)
    {
        return Ok(RevertOutcome::Changed);
    }
    match &journaled.previous {
        Some(previous) => store.set_secret(&journaled.account, previous)?,
        None => {
            store.delete_secret(&journaled.account)?;
        }
    }
    Ok(RevertOutcome::Restored)
}

/// The journal of `sync`, or `SyncNotFound` when the daemon keeps another
/// sync's or none.
fn journal_of(store: &dyn CredentialStore, sync: &SyncId, now: u64) -> Result<Journal, RpcError> {
    journal::load(store, now)
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

fn failed(message: &str) -> ImportOutcome {
    ImportOutcome::Failed {
        message: message.to_owned(),
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
