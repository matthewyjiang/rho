use std::{
    fs,
    sync::{Mutex, OnceLock},
};

use pretty_assertions::assert_eq;

use super::*;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

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
fn needs_choice_only_when_unset_and_no_env() {
    assert!(needs_explicit_choice_from(false, None));
    assert!(!needs_explicit_choice_from(true, None));
    assert!(!needs_explicit_choice_from(
        false,
        Some(CredentialStoreBackend::Os)
    ));
    assert!(!needs_explicit_choice_from(
        true,
        Some(CredentialStoreBackend::File)
    ));
}

#[test]
fn set_backend_persists_in_config() {
    let _guard = env_lock();
    let rho_dir = tempfile::tempdir().unwrap();
    let path = rho_dir.path().join("config.toml");
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

    let previous = std::env::var_os("RHO_HOME");
    std::env::set_var("RHO_HOME", rho_dir.path());

    // Use the file backend so CI hosts without an OS keyring still exercise
    // persistence + activation.
    let result = std::panic::catch_unwind(|| {
        let saved = set_backend(CredentialStoreBackend::File, Some(path.clone())).unwrap();
        assert_eq!(saved, path);

        let config = Config::load_settings_only(path.clone()).unwrap();
        assert_eq!(config.credential_store, Some(CredentialStoreBackend::File));
        let text = fs::read_to_string(saved).unwrap();
        assert!(text.contains("credential_store = \"file\""));
    });

    match previous {
        Some(value) => std::env::set_var("RHO_HOME", value),
        None => std::env::remove_var("RHO_HOME"),
    }
    result.unwrap();
}

#[test]
fn saved_policy_backend_reads_legacy_without_deleting() {
    let _guard = env_lock();
    let rho_dir = tempfile::tempdir().unwrap();
    let config_path = rho_dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
"#,
    )
    .unwrap();
    let legacy_path = rho_dir.path().join(LEGACY_POLICY_FILE);
    fs::write(&legacy_path, "file\n").unwrap();

    let previous = std::env::var_os("RHO_HOME");
    std::env::set_var("RHO_HOME", rho_dir.path());

    let result = std::panic::catch_unwind(|| {
        let backend = saved_policy_backend(Some(&config_path)).unwrap();
        assert_eq!(backend, Some(CredentialStoreBackend::File));
        assert!(
            legacy_path.exists(),
            "status/read must not delete legacy policy"
        );
    });

    match previous {
        Some(value) => std::env::set_var("RHO_HOME", value),
        None => std::env::remove_var("RHO_HOME"),
    }
    result.unwrap();
}

#[test]
fn initialize_from_config_migrates_legacy_policy_into_config() {
    let _guard = env_lock();
    let rho_dir = tempfile::tempdir().unwrap();
    let config_path = rho_dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
"#,
    )
    .unwrap();
    let legacy_path = rho_dir.path().join(LEGACY_POLICY_FILE);
    fs::write(&legacy_path, "file\n").unwrap();

    let previous = std::env::var_os("RHO_HOME");
    std::env::set_var("RHO_HOME", rho_dir.path());

    let result = std::panic::catch_unwind(|| {
        let mut config = Config::load_settings_only(config_path.clone()).unwrap();
        assert!(config.credential_store.is_none());
        initialize_from_config(&mut config, &config_path).unwrap();
        assert_eq!(config.credential_store, Some(CredentialStoreBackend::File));
        assert!(
            !legacy_path.exists(),
            "startup migration removes legacy file"
        );
        let reloaded = Config::load_settings_only(config_path.clone()).unwrap();
        assert_eq!(
            reloaded.credential_store,
            Some(CredentialStoreBackend::File)
        );
    });

    match previous {
        Some(value) => std::env::set_var("RHO_HOME", value),
        None => std::env::remove_var("RHO_HOME"),
    }
    result.unwrap();
}
