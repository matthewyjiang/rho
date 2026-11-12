use std::{
    fs::File,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{lock_account_file, lock_path_in};

// Covers: a different uid must not be able to plant or redirect the
// transaction lock through a writable home, public file, or symlink.
// Owner: account lock filesystem boundary (Unix), which guards MCP tokens.
#[test]
fn an_account_lock_requires_a_private_location() {
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
        let path = root.path().join(".rho-test.lock");
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
        let locked = lock_path_in(root.path(), owner, ".rho-test.lock", "test")
            .and_then(|path| lock_account_file(&path, "test"));
        assert_eq!(
            locked.is_ok(),
            allowed,
            "case: {case:?}, result: {locked:?}"
        );
    }
}
