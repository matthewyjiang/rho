use pretty_assertions::assert_eq;
use rho_protocol::{
    ErrorCode, ImportOutcome, ImportedSecret, RevertOutcome, SecretItem, SyncId, SyncItem,
};
use rho_providers::credentials::{CredentialStore, MemoryCredentialStore};

use super::*;
use crate::app::credential_writes::api_key_account;
use crate::app::daemon_host::sync::{journal::JOURNAL_LIFETIME, test_stores::Refusing};

const NOW: u64 = 1_000;
const ANTHROPIC: &str = "anthropic-api-key";
const MOONSHOT: &str = "moonshot-api-key";

fn account(auth: &str) -> &'static str {
    api_key_account(auth).unwrap()
}

fn saved(store: &MemoryCredentialStore, auth: &str) -> Option<String> {
    store.get_secret(account(auth)).unwrap()
}

/// One memory store for both the keys and the journal.
fn stores(store: &dyn CredentialStore) -> Stores<'_> {
    Stores {
        keys: store,
        journal: store,
    }
}

/// Imports `keys` under `sync`, writing the keys to `keys_store`.
fn import_into(
    keys_store: &dyn CredentialStore,
    journal: &MemoryCredentialStore,
    sync: &SyncId,
    keys: &[(&str, &str)],
) -> Vec<ImportedSecret> {
    let items = keys
        .iter()
        .map(|(auth, _)| SecretItem::api_key(*auth))
        .collect();
    let values: Vec<String> = keys.iter().map(|(_, key)| (*key).to_owned()).collect();
    let stores = Stores {
        keys: keys_store,
        journal,
    };
    import(stores, sync, items, &values, NOW).unwrap()
}

fn import_keys(
    store: &MemoryCredentialStore,
    sync: &SyncId,
    keys: &[(&str, &str)],
) -> Vec<ImportedSecret> {
    import_into(store, store, sync, keys)
}

/// Whether `auth`'s key still holds what `sync` wrote.
fn unchanged(store: &MemoryCredentialStore, sync: &SyncId, auth: &str) -> bool {
    let journal = journal_of(stores(store), sync, NOW).unwrap();
    let current = current(store, account(auth)).unwrap();
    journal
        .find(&SyncItem::api_key(auth))
        .unwrap()
        .unchanged(current.as_deref().map(String::as_str))
}

/// Restores `auth`'s key from `sync`'s journal.
fn restore_key(
    store: &MemoryCredentialStore,
    sync: &SyncId,
    auth: &str,
    overwrite: bool,
) -> RevertOutcome {
    let journal = journal_of(stores(store), sync, NOW).unwrap();
    let journaled = journal.find(&SyncItem::api_key(auth)).unwrap();
    restore(store, account(auth), journaled, overwrite).unwrap()
}

fn error_code<T>(result: Result<T, RpcError>) -> Option<ErrorCode> {
    result.err().and_then(|error| error.error_code())
}

// Covers: an import saves each key and journals what it replaced, keeping
// the value from before the sync's first write when the sync imports a key
// twice; restoring puts the old key back and deletes a key the sync added.
// Owner: sync keys policy (`sync/keys.rs`).
#[test]
fn a_restore_puts_back_what_the_sync_replaced() {
    let store = MemoryCredentialStore::default();
    store.set_secret(account(ANTHROPIC), "sk-old").unwrap();
    let sync = SyncId::new("s");

    import_keys(&store, &sync, &[(ANTHROPIC, "sk-first")]);
    assert_eq!(
        import_keys(
            &store,
            &sync,
            &[(ANTHROPIC, "sk-new"), (MOONSHOT, "sk-added")]
        ),
        vec![
            ImportedSecret::new(SecretItem::api_key(ANTHROPIC), ImportOutcome::Saved),
            ImportedSecret::new(SecretItem::api_key(MOONSHOT), ImportOutcome::Saved),
        ]
    );
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-new"));
    assert_eq!(
        journal_of(stores(&store), &sync, NOW).unwrap().expires_at,
        NOW + JOURNAL_LIFETIME
    );
    assert!(unchanged(&store, &sync, ANTHROPIC));
    assert!(unchanged(&store, &sync, MOONSHOT));

    assert_eq!(
        [ANTHROPIC, MOONSHOT].map(|auth| restore_key(&store, &sync, auth, /*overwrite*/ false)),
        [RevertOutcome::Restored, RevertOutcome::Restored]
    );
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-old"));
    assert_eq!(saved(&store, MOONSHOT), None);
}

// Covers: a key changed on the target since the sync reads as changed, and
// a restore leaves it unless told to overwrite.
// Owner: sync keys policy (`sync/keys.rs`).
#[test]
fn a_restore_keeps_a_key_changed_since_the_sync_unless_overwritten() {
    let store = MemoryCredentialStore::default();
    store.set_secret(account(ANTHROPIC), "sk-old").unwrap();
    let sync = SyncId::new("s");
    import_keys(&store, &sync, &[(ANTHROPIC, "sk-new")]);
    store.set_secret(account(ANTHROPIC), "sk-mine").unwrap();

    assert!(!unchanged(&store, &sync, ANTHROPIC));
    assert_eq!(
        restore_key(&store, &sync, ANTHROPIC, /*overwrite*/ false),
        RevertOutcome::Changed
    );
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-mine"));
    assert_eq!(
        restore_key(&store, &sync, ANTHROPIC, /*overwrite*/ true),
        RevertOutcome::Restored
    );
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-old"));
}

// Covers: an import saves nothing, and journals nothing, for a blank key
// or a profile the target does not know, and says so per item.
// Owner: sync keys policy (`sync/keys.rs`).
#[test]
fn an_import_skips_what_it_cannot_save() {
    let store = MemoryCredentialStore::default();
    assert_eq!(
        import_keys(
            &store,
            &SyncId::new("s"),
            &[(ANTHROPIC, "  "), ("no-such-auth", "sk-c")]
        ),
        vec![
            ImportedSecret::new(
                SecretItem::api_key(ANTHROPIC),
                ImportOutcome::Failed {
                    message: CredentialWriteError::BlankKey.to_string()
                }
            ),
            ImportedSecret::new(
                SecretItem::api_key("no-such-auth"),
                ImportOutcome::Failed {
                    message: "unknown auth profile 'no-such-auth'".into()
                }
            ),
        ]
    );
    assert_eq!(saved(&store, ANTHROPIC), None);
    assert_eq!(store.get_secret(journal::JOURNAL_ACCOUNT).unwrap(), None);
}

// Covers: a key the store refuses to write fails alone, without the key in
// its message, and leaves its journal entry as it was before the import;
// an import under a new sync that writes nothing leaves the earlier sync's
// journal revertible.
// Owner: sync keys policy (`sync/keys.rs`).
#[test]
fn a_refused_write_replaces_nothing_in_the_journal() {
    let store = MemoryCredentialStore::default();
    store.set_secret(account(ANTHROPIC), "sk-old").unwrap();
    let first = SyncId::new("first");
    import_keys(&store, &first, &[(ANTHROPIC, "sk-1")]);
    let refusing = Refusing {
        inner: &store,
        account: account(MOONSHOT),
    };
    let saved_each = |imported: Vec<ImportedSecret>| -> Vec<bool> {
        imported
            .iter()
            .map(|imported| match &imported.outcome {
                ImportOutcome::Saved => true,
                ImportOutcome::Failed { message } => {
                    assert!(!message.contains("sk-m"), "{message}");
                    false
                }
                outcome => panic!("{outcome:?}"),
            })
            .collect()
    };

    let second = import_into(
        &refusing,
        &store,
        &SyncId::new("second"),
        &[(MOONSHOT, "sk-m")],
    );
    assert_eq!(saved_each(second), vec![false]);
    let both = import_into(
        &refusing,
        &store,
        &first,
        &[(ANTHROPIC, "sk-2"), (MOONSHOT, "sk-m")],
    );
    assert_eq!(saved_each(both), vec![true, false]);

    let journal = journal_of(stores(&store), &first, NOW).unwrap();
    assert!(journal.find(&SyncItem::api_key(MOONSHOT)).is_none());
    assert!(unchanged(&store, &first, ANTHROPIC));
    restore_key(&store, &first, ANTHROPIC, /*overwrite*/ false);
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-old"));
}

// Covers: the journal holds only the last sync, so an import under another
// sync makes the earlier one not found, and it expires a lifetime after
// the sync's last write, when reading it deletes it.
// Owner: sync journal (`sync/journal.rs`, through `sync/keys.rs`).
#[test]
fn the_journal_keeps_only_the_last_sync_until_it_expires() {
    let store = MemoryCredentialStore::default();
    let (first, second) = (SyncId::new("first"), SyncId::new("second"));
    import_keys(&store, &first, &[(ANTHROPIC, "sk-1")]);
    import_keys(&store, &second, &[(ANTHROPIC, "sk-2")]);

    assert_eq!(
        error_code(discard(stores(&store), &first, NOW)),
        Some(ErrorCode::SyncNotFound)
    );
    assert!(journal_of(stores(&store), &second, NOW + JOURNAL_LIFETIME - 1).is_ok());
    assert_eq!(
        error_code(journal_of(stores(&store), &second, NOW + JOURNAL_LIFETIME)),
        Some(ErrorCode::SyncNotFound)
    );
    assert_eq!(store.get_secret(journal::JOURNAL_ACCOUNT).unwrap(), None);
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-2"));
}
