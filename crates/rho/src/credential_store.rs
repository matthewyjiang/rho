use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use rho_providers::credentials::{
    open_credential_store, probe_credential_store, CredentialError, CredentialResult,
    CredentialStore, CredentialStoreBackend, CredentialStoreProbe,
};

use crate::config::Config;

const LEGACY_POLICY_FILE: &str = "credential-store";
const ENV_BACKEND: &str = "RHO_CREDENTIAL_STORE";

/// Application credential adapter selected by the configured backend.
///
/// Backend resolution order:
/// 1. `RHO_CREDENTIAL_STORE` for the current process
/// 2. `behavior.credential_store` in config.toml
/// 3. OS store (default when unset)
///
/// File storage must be chosen explicitly through `/login`, the CLI, config, or env.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AppCredentialStore;

struct SelectedStore {
    store: Arc<dyn CredentialStore>,
}

impl CredentialStore for AppCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        selected_store()?.get_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        selected_store()?.set_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        selected_store()?.delete_secret(account)
    }
}

pub(crate) fn probe(backend: CredentialStoreBackend) -> CredentialStoreProbe {
    probe_credential_store(backend)
}

/// Backend saved in config, ignoring env overrides.
///
/// `None` means the setting is unset and the OS store is used by default.
pub(crate) fn saved_policy_backend(
    config_path: Option<&Path>,
) -> anyhow::Result<Option<CredentialStoreBackend>> {
    let path = match config_path {
        Some(path) => path.to_path_buf(),
        None => Config::default_path()?,
    };
    if !path.exists() {
        return Ok(absorb_legacy_policy_only()?);
    }
    let config = Config::load_settings_only(path)?;
    if config.credential_store.is_some() {
        return Ok(config.credential_store);
    }
    Ok(absorb_legacy_policy_only()?)
}

/// Persist the backend in config.toml and open it for this process.
pub(crate) fn set_backend(
    backend: CredentialStoreBackend,
    config_path: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    if backend == CredentialStoreBackend::File {
        let probe = probe_credential_store(backend);
        if !probe.available {
            anyhow::bail!(probe.detail);
        }
    }

    let path = match config_path {
        Some(path) => path,
        None => Config::default_path()?,
    };
    let mut config = if path.exists() {
        Config::load_settings_only(path.clone())?
    } else {
        Config::default()
    };
    config.credential_store = Some(backend);
    config.write_settings(path.clone())?;
    activate(backend)?;
    let _ = remove_legacy_policy_file();
    Ok(path)
}

/// Resolve and open the backend for this process from config + env.
pub(crate) fn bootstrap_from_config(config: &mut Config, config_path: &Path) -> anyhow::Result<()> {
    let mut dirty = absorb_legacy_policy(config)?;
    let backend = resolve_backend(config.credential_store)?;
    activate(backend)?;
    if matches!(
        config.migrate_legacy_web_search_credentials(&AppCredentialStore),
        Ok(true)
    ) {
        dirty = true;
    }
    if dirty {
        config.write_settings(config_path.to_path_buf())?;
    }
    Ok(())
}

pub(crate) fn initialize() -> CredentialResult<()> {
    selected_store().map(|_| ())
}

/// Whether config still needs an explicit credential-store choice before login.
pub(crate) fn needs_explicit_choice(config: &Config) -> bool {
    std::env::var(ENV_BACKEND)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .is_none()
        && config.credential_store.is_none()
}

pub(crate) fn configured_backend() -> CredentialResult<CredentialStoreBackend> {
    resolve_backend(read_config_backend().ok().flatten())
}

pub(crate) fn build_provider(
    provider: &str,
    model: &str,
    reasoning: rho_providers::reasoning::ReasoningLevel,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, rho_providers::model::ModelError> {
    let options = rho_providers::providers::ProviderBuildOptions::new(provider, model, reasoning)?;
    let credentials = rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(AppCredentialStore),
    );
    rho_providers::providers::build_sdk_provider_with_source(options, &credentials)
}

fn resolve_backend(
    config_backend: Option<CredentialStoreBackend>,
) -> CredentialResult<CredentialStoreBackend> {
    if let Some(value) = std::env::var(ENV_BACKEND)
        .ok()
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return CredentialStoreBackend::parse(value);
    }
    Ok(config_backend.unwrap_or(CredentialStoreBackend::Os))
}

fn activate(backend: CredentialStoreBackend) -> CredentialResult<()> {
    let store = open_credential_store(backend)?;
    let mut guard = selected_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *guard = Some(SelectedStore { store });
    Ok(())
}

fn selected_store() -> CredentialResult<Arc<dyn CredentialStore>> {
    {
        let guard = selected_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(selected) = guard.as_ref() {
            return Ok(Arc::clone(&selected.store));
        }
    }
    let backend = configured_backend()?;
    activate(backend)?;
    let guard = selected_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    guard
        .as_ref()
        .map(|selected| Arc::clone(&selected.store))
        .ok_or_else(|| {
            CredentialError::StoreUnavailable("credential store failed to initialize".into())
        })
}

fn selected_state() -> &'static Mutex<Option<SelectedStore>> {
    static STATE: OnceLock<Mutex<Option<SelectedStore>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn read_config_backend() -> anyhow::Result<Option<CredentialStoreBackend>> {
    let path = Config::default_path()?;
    if !path.exists() {
        return Ok(None);
    }
    Ok(Config::load_settings_only(path)?.credential_store)
}

fn absorb_legacy_policy(config: &mut Config) -> anyhow::Result<bool> {
    let Some(backend) = read_legacy_policy_file()? else {
        return Ok(false);
    };
    let changed = if config.credential_store.is_none() {
        config.credential_store = Some(backend);
        true
    } else {
        false
    };
    let _ = remove_legacy_policy_file();
    Ok(changed)
}

fn absorb_legacy_policy_only() -> anyhow::Result<Option<CredentialStoreBackend>> {
    let backend = read_legacy_policy_file()?;
    if backend.is_some() {
        let _ = remove_legacy_policy_file();
    }
    Ok(backend)
}

fn read_legacy_policy_file() -> CredentialResult<Option<CredentialStoreBackend>> {
    let path = match legacy_policy_path() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    if !path.exists() {
        return Ok(None);
    }
    let value = std::fs::read_to_string(&path).map_err(|error| {
        CredentialError::StoreUnavailable(format!(
            "could not read legacy credential-store policy {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(CredentialStoreBackend::parse(&value)?))
}

fn remove_legacy_policy_file() -> anyhow::Result<()> {
    let path = legacy_policy_path()?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn legacy_policy_path() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join(LEGACY_POLICY_FILE))
}

#[cfg(test)]
fn resolve_backend_from(
    environment: Option<&str>,
    config_backend: Option<CredentialStoreBackend>,
) -> CredentialResult<CredentialStoreBackend> {
    if let Some(value) = environment.map(str::trim).filter(|value| !value.is_empty()) {
        return CredentialStoreBackend::parse(value);
    }
    Ok(config_backend.unwrap_or(CredentialStoreBackend::Os))
}

#[cfg(test)]
#[path = "credential_store_tests.rs"]
mod tests;
