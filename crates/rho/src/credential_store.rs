use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use rho_providers::credentials::{
    load_web_search_api_key, open_credential_store, probe_credential_store,
    save_web_search_api_key, CredentialError, CredentialResult, CredentialStore,
    CredentialStoreBackend, CredentialStoreProbe, OsCredentialStore, WebSearchCredential,
};

use crate::config::Config;

mod client;
mod selection;
mod startup_notice;
pub(crate) mod wire;
pub(crate) use client::ClientCredentialStore;
pub(crate) use selection::{SelectedBy, StoreSelection};
pub(crate) use startup_notice::{notifying_store, StartupKeyringNotice};

const LEGACY_POLICY_FILE: &str = "credential-store";
/// The variable that chooses a process's credential store over config.
pub(crate) const ENV_BACKEND: &str = "RHO_CREDENTIAL_STORE";

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
        tracked_set(selected_store()?.as_ref(), account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        tracked_delete(selected_store()?.as_ref(), account)
    }
}

/// Saves `secret` in `store` and moves the credential generation.
fn tracked_set(store: &dyn CredentialStore, account: &str, secret: &str) -> CredentialResult<()> {
    let saved = store.set_secret(account, secret);
    // A failed write may still have changed the store, as an OS store
    // that wrote part of an entry does, so every attempt counts.
    AppCredentialStore::note_changed();
    saved
}

/// Deletes `account` from `store` and moves the credential generation when
/// that may have changed something.
fn tracked_delete(store: &dyn CredentialStore, account: &str) -> CredentialResult<bool> {
    let deleted = store.delete_secret(account);
    // Deleting nothing changed nothing a provider could have read; a
    // failed delete may have removed part of an entry.
    if !matches!(deleted, Ok(false)) {
        AppCredentialStore::note_changed();
    }
    deleted
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
    let config = Config::load(Some(path))?;
    if config.credential_store.is_some() {
        return Ok(config.credential_store);
    }
    Ok(read_legacy_policy_file()?)
}

/// Saves the web-search keys a 2.x config held in plaintext, keeping any
/// key already stored, and returns the credentials whose stored key
/// differs and was kept. Writes to the store `config_backend` (the upgraded
/// file's, else the legacy policy file's) and the environment select,
/// opened for this call only: startup opens the process's store later.
pub(crate) fn save_migrated_web_search_keys(
    config_backend: Option<CredentialStoreBackend>,
    keys: &[(WebSearchCredential, String)],
) -> anyhow::Result<Vec<WebSearchCredential>> {
    let config_backend = match config_backend {
        Some(backend) => Some(backend),
        None => read_legacy_policy_file()?,
    };
    let store = open_credential_store(resolve_backend(config_backend)?)?;
    let mut kept = Vec::new();
    for (credential, key) in keys {
        match load_web_search_api_key(store.as_ref(), *credential)? {
            None => save_web_search_api_key(store.as_ref(), *credential, key)?,
            Some(stored) if stored == *key => {}
            Some(_) => kept.push(*credential),
        }
    }
    Ok(kept)
}

/// Why [`set_backend`] did not switch stores.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SetBackendError {
    /// The store did not take a temporary secret; why, from the probe.
    #[error("{0}")]
    Unavailable(String),
    /// Config could not be saved, or the store could not be opened.
    #[error("{0:#}")]
    Save(anyhow::Error),
}

/// Persist the backend in config.toml and open it for this process, once
/// it took a temporary secret.
pub(crate) fn set_backend(
    backend: CredentialStoreBackend,
    config_path: Option<PathBuf>,
) -> Result<PathBuf, SetBackendError> {
    let availability = probe(backend);
    if !availability.available {
        return Err(SetBackendError::Unavailable(availability.detail));
    }
    save_backend(backend, config_path).map_err(SetBackendError::Save)
}

fn save_backend(
    backend: CredentialStoreBackend,
    config_path: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let path = match config_path {
        Some(path) => path,
        None => Config::default_path()?,
    };
    Config::edit(&path, |config| config.credential_store = Some(backend))?;
    remember_config_path(&path);
    activate(backend)?;
    remove_legacy_policy_file()?;
    Ok(path)
}

/// Resolve and open the backend once for this process from config + env.
///
/// Also absorbs the legacy policy file and promotes a keyless custom host
/// whose key is stored, saving only those keys. Call at startup, not on
/// every settings read.
pub(crate) fn initialize_from_config(
    config: &mut Config,
    config_path: &Path,
) -> anyhow::Result<()> {
    remember_config_path(config_path);
    let absorbed = absorb_legacy_policy(config)?;
    let backend = resolve_backend(config.credential_store)?;
    activate(backend)?;
    let promoted = config.promote_stored_custom_auth(&AppCredentialStore);
    if absorbed || promoted {
        let (backend, provider, auth) = (
            config.credential_store,
            config.provider.clone(),
            config.auth.clone(),
        );
        Config::edit(config_path, |saved| {
            saved.credential_store = backend;
            // A CLI override may have picked another provider; promote only
            // the saved one.
            if promoted && saved.provider == provider {
                saved.auth = auth;
            }
        })?;
    }
    Ok(())
}

/// Opens the backend `config` selects inside a running process: for a
/// sign-in or sign-out (see `app::credential_writes`), and for a session the
/// daemon opens, whose config may have chosen another store since the daemon
/// started. Unlike [`initialize_from_config`] it runs no migrations and
/// writes nothing, so it is safe beside startup or a live session. A store
/// that cannot be opened closes the one open before, so reads fail with the
/// same error instead of going on in a store config no longer selects.
pub(crate) fn activate_from_config(config: &Config, config_path: &Path) -> anyhow::Result<()> {
    remember_config_path(config_path);
    let opened = resolve_backend(config.credential_store).and_then(activate);
    if opened.is_err() {
        process_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .store = None;
    }
    Ok(opened?)
}

/// Build a choice request when login still needs an explicit backend
/// selection: nothing chose one. An unknown `RHO_CREDENTIAL_STORE` counts as
/// a choice, which every store access then refuses.
pub(crate) fn choice_request(config: &Config) -> Option<StoreChoiceRequest> {
    let unchosen = environment_backend().is_ok_and(|environment| {
        StoreSelection::resolve(environment, config.credential_store).selected_by
            == SelectedBy::Default
    });
    unchosen.then(StoreChoiceRequest::probe)
}

pub(crate) fn configured_backend() -> CredentialResult<CredentialStoreBackend> {
    resolve_backend(read_config_backend().ok().flatten())
}

/// `RHO_CREDENTIAL_STORE` in this process's environment, parsed; `None`
/// when it is unset or blank.
pub(crate) fn environment_backend() -> CredentialResult<Option<CredentialStoreBackend>> {
    selection::parse_environment(std::env::var(ENV_BACKEND).ok().as_deref())
}

/// The store this process selects with `environment`, its parsed
/// [`environment_backend`], and the config as it is on disk now. A config
/// that cannot be read selects nothing, as for every credential read.
pub(crate) fn current_selection(environment: Option<CredentialStoreBackend>) -> StoreSelection {
    StoreSelection::resolve(environment, read_config_backend().ok().flatten())
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

/// A provider for `model` signed in with `credentials`: those of whoever the
/// provider acts for, a session's own inside one (see
/// [`crate::session_environment::SessionEnvironment::credentials`]).
pub(crate) async fn build_provider(
    provider: &str,
    model: &str,
    reasoning: rho_providers::reasoning::ReasoningLevel,
    auth: &str,
    credentials: Arc<dyn CredentialStore>,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, rho_providers::model::ModelError> {
    build_provider_on(
        &Config::default(),
        provider,
        model,
        reasoning,
        auth,
        credentials,
    )
    .await
}

/// [`build_provider`] on `base`'s `[providers]`, so a configured endpoint or
/// custom provider reaches the model. Nothing else carries over from `base`.
pub(crate) async fn build_provider_on(
    base: &Config,
    provider: &str,
    model: &str,
    reasoning: rho_providers::reasoning::ReasoningLevel,
    auth: &str,
    credentials: Arc<dyn CredentialStore>,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, rho_providers::model::ModelError> {
    let config = Config {
        provider: provider.into(),
        model: model.into(),
        reasoning,
        auth: auth.into(),
        providers: base.providers.clone(),
        ..Config::default()
    };
    build_provider_from_config_ensuring_catalog(&config, credentials).await
}

fn resolve_backend(
    config_backend: Option<CredentialStoreBackend>,
) -> CredentialResult<CredentialStoreBackend> {
    Ok(StoreSelection::resolve(environment_backend()?, config_backend).backend)
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
    Ok(Config::load(Some(path))?.credential_store)
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
#[path = "credential_store_tests.rs"]
mod tests;
