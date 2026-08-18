//! User-defined OpenAI-compatible Chat Completions hosts.
//!
//! Names and endpoints come from application config. Each name is its own
//! provider (`/model composer/...`, `/model vllm/...`). The default auth is
//! keyless (`none`). An optional `{name}-api-key` mode stores a Bearer token.
//!
//! Descriptors are interned for `'static` lookup. Visibility is scoped: the
//! process-wide active set is the foreground config, and a runtime can overlay
//! its own names on the current thread or Tokio task without replacing that set.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

use super::{
    AuthMode, CatalogConstruction, CatalogReasoningPolicy, ModelIdCodec, ProviderAuthKind,
    ProviderDescriptor, ProviderId, ProviderModelRefreshKind, ProviderModelSource, ProviderRuntime,
    OPENAI_COMPATIBLE_API_BASE, PROVIDERS,
};

const CUSTOM_NONE_AUTH: AuthMode = AuthMode {
    id: "none",
    login_label: "No authentication required",
    auth_kind: ProviderAuthKind::None,
};

/// Auth profile id for a named custom host's optional API key.
pub fn custom_provider_api_key_auth_id(name: &str) -> String {
    format!("{name}-api-key")
}

/// Credential-store account for a named custom host's optional API key.
pub fn custom_provider_api_key_account(name: &str) -> String {
    format!("provider:{name}:api-key")
}

/// Environment override for a named custom host's optional API key.
pub fn custom_provider_api_key_env_var(name: &str) -> String {
    format!(
        "RHO_{}_API_KEY",
        name.to_ascii_uppercase().replace('-', "_")
    )
}

fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
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
pub fn install_custom_openai_compatible_providers<'a, I>(names: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    let interned = intern_custom_openai_compatible_providers(names)?;
    let mut registry = lock_write();
    registry.active = interned
        .iter()
        .filter_map(|name| registry.interned.get(name.as_str()).copied())
        .collect();
    Ok(())
}

/// Interns names without changing the process-wide active set.
pub fn intern_custom_openai_compatible_providers<'a, I>(names: I) -> anyhow::Result<Arc<[String]>>
where
    I: IntoIterator<Item = &'a str>,
{
    let names = names.into_iter().collect::<Vec<_>>();
    let mut seen = BTreeMap::<&str, ()>::new();
    for name in &names {
        validate_custom_provider_name(name)?;
        if seen.insert(*name, ()).is_some() {
            anyhow::bail!("duplicate custom provider '{name}'");
        }
    }

    let mut registry = lock_write();
    let mut interned = Vec::with_capacity(names.len());
    for name in names {
        intern(name, &mut registry);
        interned.push(name.to_string());
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

/// Drops the process-wide active set. Interned descriptors stay so parallel
/// tests can still resolve unknown-effort policy for a previously interned name.
#[doc(hidden)]
pub fn reset_custom_openai_compatible_providers_for_tests() {
    lock_write().active.clear();
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

fn intern(name: &str, registry: &mut CustomRegistry) -> &'static ProviderDescriptor {
    if let Some(existing) = registry.interned.get(name) {
        return existing;
    }
    let leaked_name = leak_str(name.to_string());
    let auth_id = leak_str(custom_provider_api_key_auth_id(name));
    let account = leak_str(custom_provider_api_key_account(name));
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
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OPENAI_COMPATIBLE_API_BASE,
            catalog_construction: CatalogConstruction::Runtime,
        },
        name: leaked_name,
        display_name: leaked_name,
        auth_modes,
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: leaked_name,
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
