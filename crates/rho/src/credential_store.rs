use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use rho_providers::credentials::{
    open_credential_store, probe_credential_store, CredentialError, CredentialResult,
    CredentialStore, CredentialStoreBackend, CredentialStoreProbe, OsCredentialStore,
};

use crate::config::Config;

mod startup_notice;
pub(crate) use startup_notice::{notifying_store, StartupKeyringNotice};

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

#[derive(Clone, Debug)]
pub(crate) struct StoreChoiceRequest {
    pub os: CredentialStoreProbe,
    pub file: CredentialStoreProbe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StoreChoiceOption {
    pub backend: CredentialStoreBackend,
    pub available: bool,
}

impl StoreChoiceRequest {
    pub fn probe() -> Self {
        Self {
            os: probe(CredentialStoreBackend::Os),
            file: probe(CredentialStoreBackend::File),
        }
    }

    pub fn options(&self) -> [StoreChoiceOption; 2] {
        [
            StoreChoiceOption {
                backend: CredentialStoreBackend::Os,
                available: self.os.available,
            },
            StoreChoiceOption {
                backend: CredentialStoreBackend::File,
                available: self.file.available,
            },
        ]
    }

    pub fn available_backends(&self) -> Vec<CredentialStoreBackend> {
        self.options()
            .into_iter()
            .filter(|option| option.available)
            .map(|option| option.backend)
            .collect()
    }

    pub fn default_backend(&self) -> Option<CredentialStoreBackend> {
        self.available_backends().into_iter().next()
    }

    pub fn detail_for(&self, backend: CredentialStoreBackend) -> &str {
        match backend {
            CredentialStoreBackend::Os => self.os.detail.as_str(),
            CredentialStoreBackend::File => self.file.detail.as_str(),
        }
    }
}

/// How many times this process changed the credentials providers build
/// with (#1662). Providers capture their key or tokens when built, so a
/// session compares the [`CredentialGeneration`] its provider read at with
/// the current one before each prompt, and rebuilds the provider once it
/// moved: a key saved or a logout made by any client of the process (another
/// ACP request, a daemon client) applies to every running session from its
/// next prompt.
///
/// One generation covers every provider auth profile, so any provider
/// credential save, logout, or OAuth refresh makes every session in the
/// process rebuild its provider at its next prompt, never mid-turn. A
/// rebuild drops provider state such as a Codex continuation, so secrets no
/// provider reads, such as MCP servers' OAuth tokens and web-search keys, go
/// through [`UntrackedCredentialStore`] and leave it alone.
///
/// It moves after every write through [`AppCredentialStore`] that may have
/// changed something, and through [`AppCredentialStore::note_changed`] for
/// login state kept outside the store. Switching to another backend does
/// not move it: providers keep what they already hold, the write that
/// follows a switch moves it, and a session whose credentials are refused
/// rereads them anyway. OAuth refreshes save rotated tokens through the
/// store too, so a refresh in one session moves it and the others pick up
/// the rotated tokens on their next prompt instead of presenting the
/// replaced ones.
/// Writes by other processes do not move it;
/// [`AppCredentialStore::reread_external_logins`] covers those once a
/// provider refuses what it holds.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A reading of the process's credential generation (see [`GENERATION`]),
/// taken before a provider reads credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CredentialGeneration(u64);

impl AppCredentialStore {
    /// Makes the next reads see logins saved by another process, such as a
    /// terminal login an editor ran. The OS store caches reads, including
    /// misses, for the life of the process; the file store always rereads.
    pub(crate) fn reread_external_logins() {
        OsCredentialStore::forget_cached_secrets();
    }

    /// The current credential generation. Take it before the build reads
    /// credentials, so a change that lands during the build still reads as
    /// after it.
    pub(crate) fn generation() -> CredentialGeneration {
        CredentialGeneration(GENERATION.load(Ordering::Acquire))
    }

    /// Records a change to what providers build with that is kept outside
    /// the credential store, such as an endpoint login recorded in config
    /// (see `app::endpoint_login`). Call once the change is written.
    pub(crate) fn note_changed() {
        GENERATION.fetch_add(1, Ordering::Release);
    }
}

impl CredentialStore for AppCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        selected_store()?.get_secret(account)
    }

    fn get_secret_fresh(&self, account: &str) -> CredentialResult<Option<String>> {
        selected_store()?.get_secret_fresh(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        let saved = selected_store()?.set_secret(account, secret);
        // A failed write may still have changed the store, as an OS store
        // that wrote part of an entry does, so every attempt counts.
        Self::note_changed();
        saved
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        let deleted = selected_store()?.delete_secret(account);
        // Deleting nothing changed nothing a provider could have read; a
        // failed delete may have removed part of an entry.
        if !matches!(deleted, Ok(false)) {
            Self::note_changed();
        }
        deleted
    }
}

/// The credential store [`AppCredentialStore`] reads and writes, for secrets
/// no model provider reads, such as MCP servers' OAuth tokens and web-search
/// keys. Its writes leave the credential generation (see [`GENERATION`])
/// alone, so they rebuild no session's provider. The caller decides that
/// what it writes feeds no provider.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UntrackedCredentialStore;

impl CredentialStore for UntrackedCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        selected_store()?.get_secret(account)
    }

    fn get_secret_fresh(&self, account: &str) -> CredentialResult<Option<String>> {
        selected_store()?.get_secret_fresh(account)
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
/// Reads are non-destructive: legacy policy files are not removed here.
pub(crate) fn saved_policy_backend(
    config_path: Option<&Path>,
) -> anyhow::Result<Option<CredentialStoreBackend>> {
    let path = match config_path {
        Some(path) => path.to_path_buf(),
        None => Config::default_path()?,
    };
    if path.exists() {
        let config = Config::load_settings_only(path)?;
        if config.credential_store.is_some() {
            return Ok(config.credential_store);
        }
    }
    Ok(read_legacy_policy_file()?)
}

/// Persist the backend in config.toml and open it for this process.
pub(crate) fn set_backend(
    backend: CredentialStoreBackend,
    config_path: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let availability = probe(backend);
    if !availability.available {
        anyhow::bail!(availability.detail);
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
    remember_config_path(&path);
    activate(backend)?;
    remove_legacy_policy_file()?;
    Ok(path)
}

/// Resolve and open the backend once for this process from config + env.
///
/// Also migrates legacy policy/web-search secrets when needed. Call at startup,
/// not on every settings read.
pub(crate) fn initialize_from_config(
    config: &mut Config,
    config_path: &Path,
) -> anyhow::Result<()> {
    remember_config_path(config_path);
    let mut dirty = absorb_legacy_policy(config)?;
    let backend = resolve_backend(config.credential_store)?;
    activate(backend)?;
    // Web-search keys feed no provider, so moving them rebuilds nothing.
    if matches!(
        config.migrate_legacy_web_search_credentials(&UntrackedCredentialStore),
        Ok(true)
    ) {
        dirty = true;
    }
    if config.promote_stored_custom_auth(&AppCredentialStore) {
        dirty = true;
    }
    if dirty {
        config.write_settings(config_path.to_path_buf())?;
    }
    Ok(())
}

/// Opens the configured backend for a sign-in or sign-out inside a running
/// process (see `app::credential_writes`). Unlike [`initialize_from_config`]
/// it runs no migrations and writes nothing, so it is safe beside startup or
/// a live session.
#[cfg(unix)]
pub(crate) fn activate_for_login(config: &Config, config_path: &Path) -> anyhow::Result<()> {
    remember_config_path(config_path);
    activate(resolve_backend(config.credential_store)?)?;
    Ok(())
}

/// Whether config still needs an explicit credential-store choice before login.
pub(crate) fn needs_explicit_choice(config: &Config) -> bool {
    env_backend_override().is_none() && config.credential_store.is_none()
}

/// Build a choice request when login still needs an explicit backend selection.
pub(crate) fn choice_request(config: &Config) -> Option<StoreChoiceRequest> {
    needs_explicit_choice(config).then(StoreChoiceRequest::probe)
}

pub(crate) fn configured_backend() -> CredentialResult<CredentialStoreBackend> {
    resolve_backend(read_config_backend().ok().flatten())
}

pub(crate) fn build_provider_from_config(
    config: &Config,
    credential_store: Arc<dyn CredentialStore>,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, rho_providers::model::ModelError> {
    let _scope = config
        .providers
        .thread_scope()
        .map_err(|error| rho_providers::model::ModelError::InvalidResponse(error.to_string()))?;
    let options = crate::app::sdk_config::provider_options_from_config(config)?;
    let credentials = rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
        credential_store,
    );
    rho_providers::providers::build_sdk_provider_with_source(options, &credentials)
}

/// Live rebuilds await catalog hydrate so npm-following providers pick the
/// advertised adapter before the first request, not after a background fetch.
///
/// Options are resolved synchronously so the thread-local custom-provider
/// overlay is never held across the catalog await.
pub(crate) async fn build_provider_from_config_ensuring_catalog(
    config: &Config,
    credential_store: Arc<dyn CredentialStore>,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, rho_providers::model::ModelError> {
    let options = crate::app::sdk_config::provider_options_from_config(config)?;
    options.ensure_catalog_for_construction().await;
    build_provider_from_config(config, credential_store)
}

pub(crate) async fn build_provider(
    provider: &str,
    model: &str,
    reasoning: rho_providers::reasoning::ReasoningLevel,
    auth: &str,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, rho_providers::model::ModelError> {
    build_provider_on(&Config::default(), provider, model, reasoning, auth).await
}

/// [`build_provider`] on `base`'s `[providers]`, so a configured endpoint or
/// custom provider reaches the model. Nothing else carries over from `base`.
pub(crate) async fn build_provider_on(
    base: &Config,
    provider: &str,
    model: &str,
    reasoning: rho_providers::reasoning::ReasoningLevel,
    auth: &str,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, rho_providers::model::ModelError> {
    let config = Config {
        provider: provider.into(),
        model: model.into(),
        reasoning,
        auth: auth.into(),
        providers: base.providers.clone(),
        ..Config::default()
    };
    build_provider_from_config_ensuring_catalog(&config, Arc::new(AppCredentialStore)).await
}

fn env_backend_override() -> Option<String> {
    std::env::var(ENV_BACKEND)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_backend(
    config_backend: Option<CredentialStoreBackend>,
) -> CredentialResult<CredentialStoreBackend> {
    if let Some(value) = env_backend_override() {
        return CredentialStoreBackend::parse(&value);
    }
    Ok(config_backend.unwrap_or(CredentialStoreBackend::Os))
}

fn activate(backend: CredentialStoreBackend) -> CredentialResult<()> {
    let store = notifying_store(open_credential_store(backend)?, backend);
    let mut guard = process_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    guard.store = Some(store);
    Ok(())
}

fn remember_config_path(path: &Path) {
    let mut guard = process_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    guard.config_path = Some(path.to_path_buf());
}

fn selected_store() -> CredentialResult<Arc<dyn CredentialStore>> {
    {
        let guard = process_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(store) = guard.store.as_ref() {
            return Ok(Arc::clone(store));
        }
    }
    let backend = configured_backend()?;
    activate(backend)?;
    let guard = process_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    guard.store.clone().ok_or_else(|| {
        CredentialError::StoreUnavailable("credential store failed to initialize".into())
    })
}

struct ProcessState {
    store: Option<Arc<dyn CredentialStore>>,
    config_path: Option<PathBuf>,
}

fn process_state() -> &'static Mutex<ProcessState> {
    static STATE: OnceLock<Mutex<ProcessState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(ProcessState {
            store: None,
            config_path: None,
        })
    })
}

fn read_config_backend() -> anyhow::Result<Option<CredentialStoreBackend>> {
    let path = {
        let guard = process_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.config_path.clone()
    };
    let path = match path {
        Some(path) => path,
        None => Config::default_path()?,
    };
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
    remove_legacy_policy_file()?;
    Ok(changed)
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
fn needs_explicit_choice_from(
    env_set: bool,
    config_backend: Option<CredentialStoreBackend>,
) -> bool {
    !env_set && config_backend.is_none()
}

#[cfg(test)]
#[path = "credential_store_tests.rs"]
mod tests;
