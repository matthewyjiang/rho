use pretty_assertions::assert_eq;
use rho_protocol::{SyncId, SyncItem};
use rho_providers::credentials::MemoryCredentialStore;

use super::*;

const NOW: u64 = 1_000;

/// The sync of the journal `store` holds, if any.
fn journaled_sync(store: &MemoryCredentialStore) -> Option<SyncId> {
    load(store, NOW).unwrap().map(|journal| journal.sync)
}

/// Starts a draft under `sync`, records a server in it, and saves it.
fn saved_draft(store: &MemoryCredentialStore, sync: &str) -> Draft {
    let mut draft = Draft::begin(store, &SyncId::new(sync), NOW).unwrap();
    let place = Place::McpServer {
        identity: "docs".into(),
    };
    draft.journal.record(place, None, Some("{}"));
    draft.save(store).unwrap();
    draft
}

// Covers: a write abandoned after its draft was saved puts back the journal
// it found, another sync's or none, so a failed write replaces nothing.
// Owner: sync journal (`sync/journal.rs`).
#[test]
fn an_abandoned_draft_puts_back_the_journal_it_found() {
    let store = MemoryCredentialStore::default();
    saved_draft(&store, "first").abandon(&store).unwrap();
    assert_eq!(journaled_sync(&store), None);

    saved_draft(&store, "first");
    let second = saved_draft(&store, "second");
    assert_eq!(journaled_sync(&store), Some(SyncId::new("second")));
    second.abandon(&store).unwrap();
    let journal = load(&store, NOW).unwrap().unwrap();
    assert_eq!(journal.sync, SyncId::new("first"));
    assert!(journal.find(&SyncItem::mcp_server("docs")).is_some());
}
