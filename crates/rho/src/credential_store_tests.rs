use std::fs;

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn defaults_to_os_without_policy() {
    assert_eq!(
        configured_backend_from(None, None).unwrap(),
        CredentialStoreBackend::Os
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
fn legacy_auto_policy_maps_to_os() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(POLICY_FILE);
    fs::write(&path, "auto\n").unwrap();

    assert_eq!(
        configured_backend_from(None, Some(&path)).unwrap(),
        CredentialStoreBackend::Os
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
    assert!(error.to_string().contains("expected os or file"));
}

#[test]
fn set_backend_writes_canonical_name() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(POLICY_FILE);

    crate::config_writer::write_atomically(
        &path,
        &format!("{}\n", CredentialStoreBackend::Os.as_str()),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap().trim(), "os");

    crate::config_writer::write_atomically(
        &path,
        &format!("{}\n", CredentialStoreBackend::File.as_str()),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap().trim(), "file");
}

#[test]
fn read_policy_backend_returns_parsed_value() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(POLICY_FILE);
    fs::write(&path, "file\n").unwrap();

    assert_eq!(
        read_policy_backend(&path).unwrap(),
        CredentialStoreBackend::File
    );
}
