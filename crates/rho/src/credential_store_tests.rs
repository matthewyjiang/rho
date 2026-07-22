use std::fs;

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn defaults_to_os_without_config_choice() {
    assert_eq!(
        resolve_backend_from(None, None).unwrap(),
        CredentialStoreBackend::Os
    );
}

#[test]
fn reads_backend_from_config_choice() {
    assert_eq!(
        resolve_backend_from(None, Some(CredentialStoreBackend::File)).unwrap(),
        CredentialStoreBackend::File
    );
}

#[test]
fn environment_overrides_config_choice() {
    assert_eq!(
        resolve_backend_from(Some("os"), Some(CredentialStoreBackend::File)).unwrap(),
        CredentialStoreBackend::Os
    );
}

#[test]
fn rejects_invalid_environment() {
    let error = resolve_backend_from(Some("plaintext-maybe"), Some(CredentialStoreBackend::File))
        .unwrap_err();
    assert!(error.to_string().contains("expected os or file"));
}

#[test]
fn set_backend_persists_in_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(
        &path,
        r#"
[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
"#,
    )
    .unwrap();

    let saved = set_backend(CredentialStoreBackend::Os, Some(path.clone())).unwrap();
    assert_eq!(saved, path);

    let config = Config::load_settings_only(path).unwrap();
    assert_eq!(config.credential_store, Some(CredentialStoreBackend::Os));
    let text = fs::read_to_string(saved).unwrap();
    assert!(text.contains("credential_store = \"os\""));
}
