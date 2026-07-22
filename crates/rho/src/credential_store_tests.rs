use std::fs;

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn defaults_to_auto_without_policy() {
    assert_eq!(
        configured_backend_from(None, None).unwrap(),
        CredentialStoreBackend::Auto
    );
}

#[test]
fn reads_backend_from_policy() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(POLICY_FILE);
    fs::write(&path, "file\n").unwrap();

    assert_eq!(
        configured_backend_from(None, Some(&path)).unwrap(),
        CredentialStoreBackend::File
    );
}

#[test]
fn environment_overrides_policy() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(POLICY_FILE);
    fs::write(&path, "file\n").unwrap();

    assert_eq!(
        configured_backend_from(Some("os"), Some(&path)).unwrap(),
        CredentialStoreBackend::Os
    );
}

#[test]
fn rejects_invalid_policy() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(POLICY_FILE);
    fs::write(&path, "plaintext-maybe\n").unwrap();

    let error = configured_backend_from(None, Some(&path)).unwrap_err();
    assert!(error.to_string().contains("expected auto, os, or file"));
}
