use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use rho_providers::credentials::{
    open_credential_store, probe_credential_store, CredentialResult, CredentialStore,
    CredentialStoreBackend, CredentialStoreProbe,
};

const POLICY_FILE: &str = "credential-store";
const ENV_BACKEND: &str = "RHO_CREDENTIAL_STORE";

/// Application credential adapter selected by the user's persisted policy.
///
/// The default and `auto` policies use only the OS credential store. File
/// storage must be selected explicitly through the installer, CLI, or env var.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AppCredentialStore;

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

pub(crate) fn has_saved_policy() -> bool {
    policy_path().is_ok_and(|path| path.exists())
}

pub(crate) fn set_backend(backend: CredentialStoreBackend) -> anyhow::Result<PathBuf> {
    let path = policy_path()?;
    crate::config_writer::write_atomically(&path, &format!("{}\n", backend.as_str()))?;
    Ok(path)
}

pub(crate) fn configured_backend() -> CredentialResult<CredentialStoreBackend> {
    configured_backend_from(
        std::env::var(ENV_BACKEND).ok().as_deref(),
        policy_path().ok().as_deref(),
    )
}

pub(crate) fn initialize() -> CredentialResult<()> {
    selected_store().map(|_| ())
}

fn selected_store() -> CredentialResult<Arc<dyn CredentialStore>> {
    static STORE: OnceLock<CredentialResult<Arc<dyn CredentialStore>>> = OnceLock::new();
    STORE
        .get_or_init(|| open_credential_store(configured_backend()?))
        .clone()
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

fn configured_backend_from(
    environment: Option<&str>,
    policy_path: Option<&std::path::Path>,
) -> CredentialResult<CredentialStoreBackend> {
    if let Some(value) = environment.filter(|value| !value.trim().is_empty()) {
        return CredentialStoreBackend::parse(value);
    }
    let Some(path) = policy_path.filter(|path| path.exists()) else {
        return Ok(CredentialStoreBackend::Auto);
    };
    let value = std::fs::read_to_string(path).map_err(|error| {
        rho_providers::credentials::CredentialError::StoreUnavailable(format!(
            "could not read credential-store policy {}: {error}",
            path.display()
        ))
    })?;
    CredentialStoreBackend::parse(&value)
}

fn policy_path() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join(POLICY_FILE))
}

#[cfg(test)]
#[path = "credential_store_tests.rs"]
mod tests;
