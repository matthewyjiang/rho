//! The sync journal: the values the last sync replaced on this daemon, so
//! `rho/sync/revert` can restore them.
//!
//! One journal, the last sync's, lives in the daemon's own credential
//! store under [`JOURNAL_ACCOUNT`], as protected as the keys it holds. Each
//! item keeps the raw store account it restores, so a revert does not need
//! the auth profile to still resolve, and a SHA-256 digest of the value the
//! sync wrote, so a revert can tell whether anything changed it since.
//!
//! The journal expires [`JOURNAL_LIFETIME`] after the sync's last write. An
//! expired journal reads as none and is deleted as it is read: at daemon
//! start, and by every `rho/sync/*` method that reads it.
//!
//! Callers pass the untracked store: no provider reads the journal, so
//! writing it must not rebuild any session's provider.

use std::{fmt, io};

use rho_protocol::{SyncId, SyncItem};
use rho_providers::credentials::{CredentialResult, CredentialStore};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// The credential store account the journal is saved as.
pub(super) const JOURNAL_ACCOUNT: &str = "sync:journal";

/// How long, in seconds, a journal outlives its sync's last write.
//
// Policy, from the approved sync design: a week, matching how long rho-ui
// keeps its snapshot of the non-secret values, so Revert means the same on
// both sides.
pub(super) const JOURNAL_LIFETIME: u64 = 7 * 24 * 60 * 60;

/// One sync's journal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Journal {
    pub(super) sync: SyncId,
    /// Unix seconds.
    pub(super) created_at: u64,
    /// Unix seconds; [`JOURNAL_LIFETIME`] after the sync's last write.
    pub(super) expires_at: u64,
    pub(super) items: Vec<JournalItem>,
}

/// What a sync changed about one item.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct JournalItem {
    #[zeroize(skip)]
    pub(super) item: SyncItem,
    /// The credential store account it is saved under.
    #[zeroize(skip)]
    pub(super) account: String,
    /// The value before the sync; `None` when the sync added it.
    pub(super) previous: Option<String>,
    /// The hex SHA-256 of the value the sync wrote.
    #[zeroize(skip)]
    pub(super) written: String,
}

impl Journal {
    /// An empty journal for `sync`, started at `now`.
    pub(super) fn new(sync: SyncId, now: u64) -> Self {
        Self {
            sync,
            created_at: now,
            expires_at: now + JOURNAL_LIFETIME,
            items: Vec::new(),
        }
    }

    /// The journaled item `item`, if the sync changed it.
    pub(super) fn find(&self, item: &SyncItem) -> Option<&JournalItem> {
        self.items.iter().find(|journaled| &journaled.item == item)
    }

    /// Records that the sync writes `value` to `item` at `account`, which
    /// holds `previous`, and returns the entry this replaces, for
    /// [`Self::undo`] if the write fails. An item the sync already changed
    /// keeps the value from before its first write.
    pub(super) fn record(
        &mut self,
        item: SyncItem,
        account: &str,
        previous: Option<String>,
        value: &str,
    ) -> Option<JournalItem> {
        let written = written(value);
        if let Some(journaled) = self
            .items
            .iter_mut()
            .find(|journaled| journaled.item == item)
        {
            let displaced = journaled.clone();
            journaled.written = written;
            return Some(displaced);
        }
        self.items.push(JournalItem {
            item,
            account: account.to_owned(),
            previous,
            written,
        });
        None
    }

    /// Puts back the entry for `item` that [`Self::record`] displaced.
    pub(super) fn undo(&mut self, item: &SyncItem, displaced: Option<JournalItem>) {
        self.remove(item);
        self.items.extend(displaced);
    }

    /// Forgets `item`, once restored.
    pub(super) fn remove(&mut self, item: &SyncItem) {
        self.items.retain(|journaled| &journaled.item != item);
    }
}

impl JournalItem {
    /// Whether `current`, the value its account holds now, is still what
    /// the sync wrote.
    pub(super) fn unchanged(&self, current: Option<&str>) -> bool {
        current.is_some_and(|current| written(current) == self.written)
    }
}

// By hand, so printing a journal never prints a key.
impl fmt::Debug for JournalItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JournalItem")
            .field("item", &self.item)
            .field("account", &self.account)
            .field("previous", &self.previous.as_ref().map(|_| "[key]"))
            .field("written", &self.written)
            .finish()
    }
}

/// The journal `store` holds, unless it expired by `now`, in which case it
/// is deleted. A journal that does not parse is deleted too: it can no
/// longer restore anything.
pub(super) fn load(store: &dyn CredentialStore, now: u64) -> CredentialResult<Option<Journal>> {
    let Some(saved) = store.get_secret_fresh(JOURNAL_ACCOUNT)?.map(Zeroizing::new) else {
        return Ok(None);
    };
    match serde_json::from_str::<Journal>(&saved) {
        Ok(journal) if journal.expires_at > now => Ok(Some(journal)),
        Ok(_) => {
            tracing::info!("deleting the expired sync journal");
            delete(store).map(|_| None)
        }
        Err(error) => {
            tracing::warn!(%error, "deleting a sync journal that does not parse");
            delete(store).map(|_| None)
        }
    }
}

/// Saves `journal`, or deletes the journal once it has no items left.
pub(super) fn save(store: &dyn CredentialStore, journal: &Journal) -> CredentialResult<()> {
    if journal.items.is_empty() {
        return delete(store);
    }
    // Sized exactly before writing, so growing the buffer leaves no
    // unzeroized copy of a key behind.
    let mut length = Length(0);
    serde_json::to_writer(&mut length, journal).expect("a journal serializes");
    let mut saved = Zeroizing::new(Vec::with_capacity(length.0));
    serde_json::to_writer(&mut *saved, journal).expect("a journal serializes");
    store.set_secret(
        JOURNAL_ACCOUNT,
        std::str::from_utf8(&saved).expect("JSON is UTF-8"),
    )
}

/// Deletes the journal, if there is one.
pub(super) fn delete(store: &dyn CredentialStore) -> CredentialResult<()> {
    store.delete_secret(JOURNAL_ACCOUNT).map(|_| ())
}

/// What the journal keeps of a value the sync wrote: its hex SHA-256.
fn written(value: &str) -> String {
    hex::encode(super::digest(value))
}

/// Counts the bytes written to it.
struct Length(usize);

impl io::Write for Length {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
