//! User-defined OpenAI-compatible hosts.
//!
//! Names and endpoints come from application config. Each name is its own
//! provider (`/model composer/...`, `/model vllm/...`). The default auth is
//! keyless (`none`). An optional `{name}-api-key` mode stores a Bearer token.
//!
//! Hosts speak Chat Completions by default, or Responses when interned with
//! [`CustomProviderOptions::api`].
//!
//! A host may borrow another models.dev slug for context, price, and reasoning
//! metadata via `catalog`; that slug becomes its `metadata_upstream`.
//! `catalog_mode = "model-id"` instead keeps the selected `slug/model` id and
//! looks that unsplit id up in a shared ExactAdvertised models.dev tree.
//!
//! Descriptors are interned for `'static` lookup. Visibility is scoped: the
//! process-wide active set is the foreground config, and a runtime can overlay
//! its own names on the current thread or Tokio task without replacing that set.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

use super::{
    AuthMode, CatalogConstruction, CatalogLookupMode, CatalogReasoningPolicy, ModelIdCodec,
    OpenAiCompatibleApi, ProviderAuthKind, ProviderDescriptor, ProviderId,
    ProviderModelRefreshKind, ProviderModelSource, ProviderRuntime, OPENAI_COMPATIBLE_API_BASE,
    PROVIDERS,
};

const CUSTOM_NONE_AUTH: AuthMode = AuthMode {
    id: super::KEYLESS_AUTH,
    login_label: "No authentication required",
    auth_kind: ProviderAuthKind::None,
};

/// Auth profile id for a named custom host's optional API key.
pub fn custom_provider_api_key_auth_id(name: &str) -> String {
    format!("{name}-api-key")
}

/// Environment override for a named custom host's optional API key.
pub(super) fn custom_provider_api_key_env_var(name: &str) -> String {
    format!(
        "RHO_{}_API_KEY",
        name.to_ascii_uppercase().replace('-', "_")
    )
}

/// True when `name` matches the `RHO_<NAME>_API_KEY` override convention.
pub(super) fn is_provider_api_key_env_var(name: &str) -> bool {
    name.starts_with("RHO_") && name.ends_with("_API_KEY")
}

/// Whether `value` is a syntactically valid `{name}-api-key` custom auth id.
///
/// The host does not have to be interned yet. CLI `--auth` uses this because
/// [`crate::auth_profiles`] is the static built-in list.
pub fn is_custom_provider_api_key_auth(value: &str) -> bool {
    value
        .strip_suffix("-api-key")
        .is_some_and(|name| validate_custom_provider_name(name).is_ok())
}

fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

/// A config-defined host: its provider name and the models.dev slug it borrows.
///
/// `catalog` is `None` when the host has no `catalog` override, in which case
/// it borrows nothing and its own name is the metadata slug.
///
/// # Next major
///
/// NEXT_MAJOR(rho-providers): fold CustomProviderOptions into CustomProviderSpec
/// and ProviderDescriptor (catalog lookup and api) so intern does not need a side
/// table, and collapse the `_with_lookup` / `_with_options` funnels.
///
/// Both types are public 1.x API and every `CustomProviderSpec` field is `pub`,
/// so new fields would break external struct literals. Until then, pass intern
/// options through [`install_custom_openai_compatible_providers_with_options`] /
/// [`intern_custom_openai_compatible_providers_with_options`] (lookup-only
/// callers can keep using the `_with_lookup` wrappers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustomProviderSpec<'a> {
    pub name: &'a str,
    pub catalog: Option<&'a str>,
}

impl<'a> CustomProviderSpec<'a> {
    pub fn new(name: &'a str, catalog: Option<&'a str>) -> Self {
        Self {
            name,
            catalog: catalog.map(str::trim).filter(|slug| !slug.is_empty()),
        }
    }

    /// models.dev slug this host reads metadata rows under.
    ///
    /// Model-id lookup ignores this and reads the unsplit `slug/model` id from
    /// the shared catalog tree instead.
    fn metadata_upstream(&self) -> &'a str {
        self.catalog.unwrap_or(self.name)
    }
}

/// A bare name is a host that borrows no catalog.
impl<'a> From<&'a str> for CustomProviderSpec<'a> {
    fn from(name: &'a str) -> Self {
        Self::new(name, None)
    }
}

/// Per-host intern options that cannot live on [`CustomProviderSpec`] in 1.x.
///
/// # Next major
///
/// NEXT_MAJOR(rho-providers): fold CustomProviderOptions into CustomProviderSpec
/// (catalog lookup and api) and collapse the `_with_lookup` / `_with_options` funnels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CustomProviderOptions {
    pub catalog_lookup: CatalogLookupMode,
    pub api: OpenAiCompatibleApi,
}

impl CustomProviderOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_catalog_lookup(mut self, catalog_lookup: CatalogLookupMode) -> Self {
        self.catalog_lookup = catalog_lookup;
        self
    }

    pub fn with_api(mut self, api: OpenAiCompatibleApi) -> Self {
        self.api = api;
        self
    }
}

#[derive(Default)]
struct CustomRegistry {
    interned: BTreeMap<String, &'static ProviderDescriptor>,
    active: Vec<&'static ProviderDescriptor>,
}

fn registry() -> &'static RwLock<CustomRegistry> {
    static REGISTRY: OnceLock<RwLock<CustomRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(CustomRegistry::default()))
}

fn lock_read() -> std::sync::RwLockReadGuard<'static, CustomRegistry> {
    registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_write() -> std::sync::RwLockWriteGuard<'static, CustomRegistry> {
    registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

thread_local! {
    static THREAD_SCOPE: RefCell<Vec<Arc<[String]>>> = const { RefCell::new(Vec::new()) };
}

tokio::task_local! {
    static TASK_SCOPE: Arc<[String]>;
}

fn scoped_names() -> Option<Arc<[String]>> {
    TASK_SCOPE
        .try_with(Arc::clone)
        .ok()
        .or_else(|| THREAD_SCOPE.with(|stack| stack.borrow().last().cloned()))
}

/// Interns names and publishes them as the process-wide picker/lookup set.
///
/// A later install replaces that process set. Concurrent runtimes that must
/// keep the foreground set intact should intern and enter a
/// [`CustomProviderThreadScope`] or [`scope_custom_openai_compatible_providers`]
/// instead of installing.
pub fn install_custom_openai_compatible_providers<'a, I>(specs: I) -> anyhow::Result<()>
where
    I: IntoIterator,
    I::Item: Into<CustomProviderSpec<'a>>,
{
    install_custom_openai_compatible_providers_with_lookup(
        specs
            .into_iter()
            .map(|spec| (spec, CatalogLookupMode::Slug)),
    )
}

/// Like [`install_custom_openai_compatible_providers`], with an explicit lookup mode.
pub fn install_custom_openai_compatible_providers_with_lookup<'a, I, S>(
    specs: I,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = (S, CatalogLookupMode)>,
    S: Into<CustomProviderSpec<'a>>,
{
    install_custom_openai_compatible_providers_with_options(
        specs
            .into_iter()
            .map(|(spec, mode)| (spec, CustomProviderOptions::new().with_catalog_lookup(mode))),
    )
}

/// Like [`install_custom_openai_compatible_providers`], with explicit intern options.
pub fn install_custom_openai_compatible_providers_with_options<'a, I, S>(
    specs: I,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = (S, CustomProviderOptions)>,
    S: Into<CustomProviderSpec<'a>>,
{
    let interned = intern_custom_openai_compatible_providers_with_options(specs)?;
    let mut registry = lock_write();
    registry.active = interned
        .iter()
        .filter_map(|name| registry.interned.get(name.as_str()).copied())
        .collect();
    Ok(())
}

/// Interns hosts without changing the process-wide active set.
pub fn intern_custom_openai_compatible_providers<'a, I>(specs: I) -> anyhow::Result<Arc<[String]>>
where
    I: IntoIterator,
    I::Item: Into<CustomProviderSpec<'a>>,
{
    intern_custom_openai_compatible_providers_with_lookup(
        specs
            .into_iter()
            .map(|spec| (spec, CatalogLookupMode::Slug)),
    )
}

/// Like [`intern_custom_openai_compatible_providers`], with an explicit lookup mode.
pub fn intern_custom_openai_compatible_providers_with_lookup<'a, I, S>(
    specs: I,
) -> anyhow::Result<Arc<[String]>>
where
    I: IntoIterator<Item = (S, CatalogLookupMode)>,
    S: Into<CustomProviderSpec<'a>>,
{
    intern_custom_openai_compatible_providers_with_options(
        specs
            .into_iter()
            .map(|(spec, mode)| (spec, CustomProviderOptions::new().with_catalog_lookup(mode))),
    )
}

/// Like [`intern_custom_openai_compatible_providers`], with explicit intern options.
pub fn intern_custom_openai_compatible_providers_with_options<'a, I, S>(
    specs: I,
) -> anyhow::Result<Arc<[String]>>
where
    I: IntoIterator<Item = (S, CustomProviderOptions)>,
    S: Into<CustomProviderSpec<'a>>,
{
    let specs = specs
        .into_iter()
        .map(|(spec, options)| (spec.into(), options))
        .collect::<Vec<(CustomProviderSpec<'a>, CustomProviderOptions)>>();
    let mut seen = BTreeMap::<&str, ()>::new();
    for (spec, _) in &specs {
        validate_custom_provider_name(spec.name)?;
        if seen.insert(spec.name, ()).is_some() {
            anyhow::bail!("duplicate custom provider '{}'", spec.name);
        }
    }

    let mut registry = lock_write();
    let mut interned = Vec::with_capacity(specs.len());
    for (spec, options) in specs {
        intern(spec, options, &mut registry);
        interned.push(spec.name.to_string());
    }
    Ok(interned.into())
}

/// Overlays custom provider visibility on the current thread until dropped.
pub struct CustomProviderThreadScope {
    _private: (),
}

impl CustomProviderThreadScope {
    pub fn enter(names: Arc<[String]>) -> Self {
        THREAD_SCOPE.with(|stack| stack.borrow_mut().push(names));
        Self { _private: () }
    }
}

impl Drop for CustomProviderThreadScope {
    fn drop(&mut self) {
        THREAD_SCOPE.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Overlays custom provider visibility on the current Tokio task.
pub async fn scope_custom_openai_compatible_providers<F, T>(names: Arc<[String]>, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    TASK_SCOPE.scope(names, future).await
}

/// Serializes tests that mutate the process-wide custom provider set.
#[doc(hidden)]
pub fn custom_provider_registry_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Drops the process-wide active set. Interned names stay so parallel tests can
/// still resolve unknown-effort policy, but leftover catalog slugs and lookup
/// modes are reset so snapshot tests do not inherit extra models.dev docs.
#[doc(hidden)]
pub fn reset_custom_openai_compatible_providers_for_tests() {
    let names = {
        let mut registry = lock_write();
        registry.active.clear();
        registry.interned.keys().cloned().collect::<Vec<_>>()
    };
    if names.is_empty() {
        return;
    }
    let specs = names
        .iter()
        .map(|name| CustomProviderSpec::new(name, None))
        .collect::<Vec<_>>();
    let _ = intern_custom_openai_compatible_providers(specs);
}

pub fn custom_openai_compatible_providers() -> Vec<&'static ProviderDescriptor> {
    if let Some(names) = scoped_names() {
        return names
            .iter()
            .filter_map(|name| interned_custom_provider(name))
            .collect();
    }
    lock_read().active.clone()
}

pub fn custom_openai_compatible_provider(name: &str) -> Option<&'static ProviderDescriptor> {
    if let Some(names) = scoped_names() {
        if !names.iter().any(|visible| visible == name) {
            return None;
        }
        return interned_custom_provider(name);
    }
    lock_read()
        .active
        .iter()
        .copied()
        .find(|descriptor| descriptor.name == name)
}

pub fn interned_custom_provider(name: &str) -> Option<&'static ProviderDescriptor> {
    lock_read().interned.get(name).copied()
}

pub(crate) fn interned_custom_providers() -> Vec<&'static ProviderDescriptor> {
    lock_read().interned.values().copied().collect()
}

pub(crate) fn interned_custom_provider_for_auth(auth: &str) -> Option<&'static ProviderDescriptor> {
    lock_read().interned.values().copied().find(|descriptor| {
        descriptor
            .auth_modes()
            .any(|mode| mode.id == auth && !matches!(mode.auth_kind, ProviderAuthKind::None))
    })
}

pub fn validate_custom_provider_name(name: &str) -> anyhow::Result<()> {
    if name == "all" {
        anyhow::bail!("custom provider name 'all' is reserved");
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        anyhow::bail!("custom provider name must not be empty");
    };
    if !first.is_ascii_lowercase()
        || !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        anyhow::bail!(
            "custom provider '{name}' must be lowercase letters, digits, and hyphens, starting with a letter"
        );
    }
    if PROVIDERS.iter().any(|descriptor| descriptor.name == name)
        || super::legacy_provider_alias(name).is_some()
    {
        anyhow::bail!("custom provider '{name}' conflicts with a built-in provider");
    }
    Ok(())
}

/// Interns one host, reusing the existing descriptor when the catalog slug,
/// lookup mode, and wire API are unchanged. A config edit that repoints any of
/// those must not keep serving the previously leaked descriptor.
fn intern(
    spec: CustomProviderSpec<'_>,
    options: CustomProviderOptions,
    registry: &mut CustomRegistry,
) -> &'static ProviderDescriptor {
    let name = spec.name;
    let metadata_upstream = spec.metadata_upstream();
    let catalog_lookup = options.catalog_lookup;
    let construction = match options.api {
        OpenAiCompatibleApi::ChatCompletions => CatalogConstruction::Runtime,
        OpenAiCompatibleApi::Responses => CatalogConstruction::Responses,
    };
    if let Some(existing) = registry.interned.get(name).copied().filter(|existing| {
        existing.metadata_upstream == metadata_upstream
            && existing.catalog_lookup() == catalog_lookup
            && existing.runtime.catalog_construction() == construction
    }) {
        return existing;
    }
    let leaked_name = leak_str(name.to_string());
    let metadata_upstream = if metadata_upstream == name {
        leaked_name
    } else {
        leak_str(metadata_upstream.to_string())
    };
    let auth_id = leak_str(custom_provider_api_key_auth_id(name));
    let account = leak_str(format!("provider:{name}:api-key"));
    let env_var = leak_str(custom_provider_api_key_env_var(name));
    let entry_label = leak_str(format!("{name} API key"));
    let missing_message = leak_str(format!(
        "missing {name} API key; run /login {name} in the TUI or set {env_var} as a CI/dev override"
    ));
    let auth_modes: &'static [AuthMode] = Box::leak(Box::new([
        CUSTOM_NONE_AUTH,
        AuthMode {
            id: auth_id,
            login_label: entry_label,
            auth_kind: ProviderAuthKind::ApiKey {
                env_var,
                account,
                entry_label,
                missing_message,
            },
        },
    ]));
    let descriptor = Box::leak(Box::new(ProviderDescriptor {
        // NEXT_MAJOR(rho-providers): add ProviderId::OpenAiCompatible so
        // config-defined hosts are not aliased onto a built-in id.
        id: ProviderId::Ollama,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Custom,
            default_api_base: OPENAI_COMPATIBLE_API_BASE,
            catalog_construction: construction,
        },
        name: leaked_name,
        display_name: leaked_name,
        auth_modes,
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        // Own name unless `catalog` borrows another models.dev slug.
        metadata_upstream,
        catalog_lookup,
        // Same Chat Completions effort field as Ollama. Custom names are not in
        // models.dev, so Unknown must still send the selected level.
        catalog_reasoning: CatalogReasoningPolicy::OffAsNone,
        default_model: None,
    }));
    registry.interned.insert(name.to_string(), descriptor);
    descriptor
}

#[cfg(test)]
#[path = "custom_openai_compatible_tests.rs"]
mod tests;
