use std::{io, path::PathBuf};

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialResult, FileCredentialStore};
use tempfile::TempDir;

use super::*;
use crate::app::mcp_server::tokens::Rotation;

struct ContendedStore {
    store: FileCredentialStore,
    lock_path: PathBuf,
}

impl ContendedStore {
    /// Another descriptor represents an independent command/process.
    fn assert_transaction_locked(&self) {
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .unwrap();
        assert_eq!(
            fs2::FileExt::try_lock_exclusive(&contender)
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::WouldBlock),
            "another token writer must be excluded for the whole transaction"
        );
    }
}

impl CredentialStore for ContendedStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.assert_transaction_locked();
        self.store.get_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.assert_transaction_locked();
        self.store.set_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        self.store.delete_secret(account)
    }
}

// Covers: another CLI writer cannot read a stale token snapshot while a
// revoke/rotation is in flight, then restore a revoked or replaced secret.
// Owner: MCP token-store transaction (cross-process lock + uncached reload).
#[test]
fn token_updates_exclude_other_writers_from_reload_through_save() {
    let root = TempDir::new().unwrap();
    let lock_path = root.path().join("tokens.lock");
    let store = ContendedStore {
        store: FileCredentialStore::with_rho_home(root.path()).unwrap(),
        lock_path,
    };
    // An existing lock file alone is not enough; the transaction must hold it.
    drop(lock_tokens(&store.lock_path).unwrap());
    let rotation = || Rotation {
        access: None,
        workspaces: vec![root.path().to_owned()],
    };
    let old = update_at(&store, &store.lock_path, |tokens| {
        tokens.rotate("compromised", rotation())
    })
    .unwrap();
    update_at(&store, &store.lock_path, |tokens| {
        store.assert_transaction_locked();
        assert!(tokens.revoke("compromised"));
        Ok(())
    })
    .unwrap();
    let new = update_at(&store, &store.lock_path, |tokens| {
        store.assert_transaction_locked();
        assert!(tokens.authenticate(&old).is_none());
        tokens.rotate("other", rotation())
    })
    .unwrap();
    let saved = TokenSet::load(&store.store).unwrap();
    assert_eq!(
        (
            saved.authenticate(&old).is_some(),
            saved.authenticate(&new).map(|token| token.name.as_str()),
        ),
        (false, Some("other"))
    );
    // The completed transaction releases the same lock to the next command.
    drop(lock_tokens(&store.lock_path).unwrap());
}
