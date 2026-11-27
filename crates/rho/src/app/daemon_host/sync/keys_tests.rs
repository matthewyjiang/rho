use pretty_assertions::assert_eq;
use rho_protocol::{
    ErrorCode, ImportOutcome, ImportedSecret, RevertOutcome, RevertedItem, SecretItem, SyncId,
    SyncItem, SyncItemState, SyncItemStatus, SyncStatusResponse,
};
use rho_providers::credentials::{CredentialStore, MemoryCredentialStore};

use super::*;
use crate::app::credential_writes::api_key_account;

const NOW: u64 = 1_000;
const ANTHROPIC: &str = "anthropic-api-key";
const MOONSHOT: &str = "moonshot-api-key";

fn account(auth: &str) -> &'static str {
    api_key_account(auth).unwrap()
}

fn saved(store: &MemoryCredentialStore, auth: &str) -> Option<String> {
    store.get_secret(account(auth)).unwrap()
}

/// Imports `keys` under `sync` with no environment override.
fn import_keys(
    store: &MemoryCredentialStore,
    sync: &SyncId,
    keys: &[(&str, &str)],
) -> Vec<ImportedSecret> {
    let items = keys
        .iter()
        .map(|(auth, _)| SecretItem::api_key(*auth))
        .collect();
    let values: Vec<String> = keys.iter().map(|(_, key)| (*key).to_owned()).collect();
    import(store, sync, items, &values, NOW, |_| None).unwrap()
}

fn error_code<T>(result: Result<T, RpcError>) -> Option<ErrorCode> {
    result.err().and_then(|error| error.error_code())
}

// Covers: an import saves each key and journals what it replaced, keeping
// the value from before the sync's first write when the sync imports a key
// twice; a revert puts the old key back, deletes a key the sync added, and
// deletes the journal once nothing is left, so the sync is then not found.
// Owner: sync keys policy (`sync/keys.rs`).
#[test]
fn a_revert_restores_what_the_sync_replaced() {
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
        status(&store, &sync, NOW).unwrap(),
        SyncStatusResponse::new(
            NOW + JOURNAL_LIFETIME,
            vec![
                SyncItemStatus::new(SyncItem::api_key(ANTHROPIC), SyncItemState::Unchanged),
                SyncItemStatus::new(SyncItem::api_key(MOONSHOT), SyncItemState::Unchanged),
            ]
        )
    );

    let both = vec![SyncItem::api_key(ANTHROPIC), SyncItem::api_key(MOONSHOT)];
    assert_eq!(
        revert(&store, &sync, both.clone(), &[], NOW).unwrap(),
        vec![
            RevertedItem::new(SyncItem::api_key(ANTHROPIC), RevertOutcome::Restored),
            RevertedItem::new(SyncItem::api_key(MOONSHOT), RevertOutcome::Restored),
        ]
    );
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-old"));
    assert_eq!(saved(&store, MOONSHOT), None);
    assert_eq!(store.get_secret(journal::JOURNAL_ACCOUNT).unwrap(), None);
    assert_eq!(
        error_code(status(&store, &sync, NOW)),
        Some(ErrorCode::SyncNotFound)
    );
}

// Covers: a key changed on the target since the sync reads as changed, and
// a revert leaves it, and keeps it journaled, unless `overwrite` names it.
// Owner: sync keys policy (`sync/keys.rs`).
#[test]
fn a_revert_keeps_a_key_changed_since_the_sync_unless_overwritten() {
    let store = MemoryCredentialStore::default();
    store.set_secret(account(ANTHROPIC), "sk-old").unwrap();
    let sync = SyncId::new("s");
    import_keys(&store, &sync, &[(ANTHROPIC, "sk-new")]);
    store.set_secret(account(ANTHROPIC), "sk-mine").unwrap();
    let item = SyncItem::api_key(ANTHROPIC);

    assert_eq!(
        status(&store, &sync, NOW).unwrap().items,
        vec![SyncItemStatus::new(item.clone(), SyncItemState::Changed)]
    );
    assert_eq!(
        revert(&store, &sync, vec![item.clone()], &[], NOW).unwrap(),
        vec![RevertedItem::new(item.clone(), RevertOutcome::Changed)]
    );
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-mine"));
    assert_eq!(
        revert(
            &store,
            &sync,
            vec![item.clone()],
            std::slice::from_ref(&item),
            NOW
        )
        .unwrap(),
        vec![RevertedItem::new(item, RevertOutcome::Restored)]
    );
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-old"));
}

// Covers: an import saves nothing, and journals nothing, for a profile a
// daemon variable overrides, a blank key, or a profile the target does not
// know, and says why per item.
// Owner: sync keys policy (`sync/keys.rs`).
#[test]
fn an_import_skips_what_it_cannot_save() {
    let store = MemoryCredentialStore::default();
    let items = vec![
        SecretItem::api_key(ANTHROPIC),
        SecretItem::api_key(MOONSHOT),
        SecretItem::api_key("no-such-auth"),
    ];
    let values = ["sk-a".to_owned(), "  ".to_owned(), "sk-c".to_owned()];
    let imported = import(&store, &SyncId::new("s"), items, &values, NOW, |name| {
        (name == "ANTHROPIC_API_KEY").then(|| "sk-env".to_owned())
    })
    .unwrap();
    assert_eq!(
        imported
            .into_iter()
            .map(|imported| imported.outcome)
            .collect::<Vec<_>>(),
        vec![
            ImportOutcome::Overridden {
                variable: "ANTHROPIC_API_KEY".into()
            },
            ImportOutcome::Failed {
                message: "the key is blank".into()
            },
            ImportOutcome::Failed {
                message: "unknown auth profile 'no-such-auth'".into()
            },
        ]
    );
    assert_eq!(saved(&store, ANTHROPIC), None);
    assert_eq!(store.get_secret(journal::JOURNAL_ACCOUNT).unwrap(), None);
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
        error_code(discard(&store, &first, NOW)),
        Some(ErrorCode::SyncNotFound)
    );
    assert!(status(&store, &second, NOW + JOURNAL_LIFETIME - 1).is_ok());
    assert_eq!(
        error_code(status(&store, &second, NOW + JOURNAL_LIFETIME)),
        Some(ErrorCode::SyncNotFound)
    );
    assert_eq!(store.get_secret(journal::JOURNAL_ACCOUNT).unwrap(), None);
    assert_eq!(saved(&store, ANTHROPIC).as_deref(), Some("sk-2"));
}
