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

// Covers: a different uid must not be able to plant or redirect the
// transaction lock through a writable home, public file, or symlink.
// Owner: MCP token mutation lock filesystem boundary.
#[test]
fn token_mutations_require_a_private_lock_location() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[derive(Debug)]
    enum Location {
        Private,
        WritableHome,
        WrongOwner,
        PublicLock,
        SymlinkLock,
    }
    for (case, allowed) in [
        (Location::Private, true),
        (Location::WritableHome, false),
        (Location::WrongOwner, false),
        (Location::PublicLock, false),
        (Location::SymlinkLock, false),
    ] {
        let root = TempDir::new().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut owner = root.path().metadata().unwrap().uid();
        let path = root.path().join(".rho-mcp-tokens.lock");
        match case {
            Location::Private => {}
            Location::WrongOwner => owner ^= 1,
            Location::WritableHome => {
                std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o770))
                    .unwrap();
            }
            Location::PublicLock => {
                File::create(&path).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            Location::SymlinkLock => {
                let target = root.path().join("target");
                File::create(&target).unwrap();
                symlink(target, &path).unwrap();
            }
        }
        let locked = lock_path_in(root.path(), owner).and_then(|path| lock_tokens(&path));
        assert_eq!(
            locked.is_ok(),
            allowed,
            "case: {case:?}, result: {locked:?}"
        );
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
