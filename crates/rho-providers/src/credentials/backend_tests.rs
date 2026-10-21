use std::sync::atomic::{AtomicUsize, Ordering};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::credentials::{FileCredentialStore, MemoryCredentialStore};

#[test]
fn parses_backend_selector_values() {
    assert_eq!(
        CredentialStoreBackend::parse("auto").unwrap(),
        CredentialStoreBackend::Os
    );
    assert_eq!(
        CredentialStoreBackend::parse("OS").unwrap(),
        CredentialStoreBackend::Os
    );
    assert_eq!(
        CredentialStoreBackend::parse(" file ").unwrap(),
        CredentialStoreBackend::File
    );
    assert!(CredentialStoreBackend::parse("sqlite").is_err());
}

#[test]
fn availability_probe_retries_cleanup_after_delete_failure() {
    struct DeleteOnceFailsStore {
        inner: MemoryCredentialStore,
        deletes: AtomicUsize,
    }

    impl CredentialStore for DeleteOnceFailsStore {
        fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
            self.inner.get_secret(account)
        }

        fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
            self.inner.set_secret(account, secret)
        }

        fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
            if self.deletes.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(CredentialError::StoreUnavailable(
                    "injected delete failure".into(),
                ));
            }
            self.inner.delete_secret(account)
        }
    }

    let store = DeleteOnceFailsStore {
        inner: MemoryCredentialStore::default(),
        deletes: AtomicUsize::new(0),
    };
    run_availability_probe(&store).unwrap();
    assert!(store
        .inner
        .get_secret("should-not-exist")
        .unwrap()
        .is_none());
}

#[test]
fn availability_probe_is_non_destructive_for_existing_secrets() {
    let store = MemoryCredentialStore::default();
    store.set_secret("keep-me", "important").unwrap();
    run_availability_probe(&store).unwrap();
    assert_eq!(
        store.get_secret("keep-me").unwrap().as_deref(),
        Some("important")
    );
}

#[test]
fn file_backend_probe_uses_explicit_file_store() {
    let root = TempDir::new().unwrap();
    let store = FileCredentialStore::with_rho_home(root.path()).unwrap();
    run_availability_probe(&store).unwrap();
    // Probe accounts use a rho-probe prefix and must not remain after success.
    let secrets = std::fs::read_to_string(store.secrets_path()).unwrap_or_default();
    assert!(!secrets.contains("rho-probe-secret:"), "{secrets}");
    assert!(!secrets.contains("\"rho-probe:"), "{secrets}");
}
