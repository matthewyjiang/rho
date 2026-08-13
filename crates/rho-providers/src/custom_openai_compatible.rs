//! User-defined OpenAI-compatible Chat Completions hosts.
//!
//! Names and endpoints come from application config. Each name is its own
//! provider (`/model composer/...`, `/model vllm/...`). They are keyless, like
//! Ollama, and do not appear in `/login`.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

use super::{
    AuthMode, CatalogReasoningPolicy, ModelIdCodec, ProviderAuthKind, ProviderDescriptor,
    ProviderId, ProviderModelRefreshKind, ProviderModelSource, ProviderRuntime,
    UnknownEffortPolicy, OPENAI_COMPATIBLE_API_BASE, PROVIDERS,
};

const CUSTOM_AUTH: &[AuthMode] = &[AuthMode {
    id: "none",
    login_label: "No authentication required",
    auth_kind: ProviderAuthKind::None,
}];

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

/// Replaces the active custom OpenAI-compatible providers.
///
/// Names are interned for the process lifetime so descriptors can be `'static`.
/// The active set is the current config: a later install drops names that are
/// no longer listed. Interned rows stay so a name can be reinstalled without
/// leaking a second descriptor. Callers that only change an endpoint do not
/// need to reinstall; the application config remains the source of truth for
/// the API base.
pub fn install_custom_openai_compatible_providers<'a, I>(names: I) -> anyhow::Result<()>
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
    let mut active = Vec::with_capacity(names.len());
    for name in names {
        active.push(intern(name, &mut registry));
    }
    registry.active = active;
    Ok(())
}

/// Drops interned and active custom providers. Tests use this so one case
/// cannot leak names into another.
#[doc(hidden)]
pub fn reset_custom_openai_compatible_providers_for_tests() {
    *lock_write() = CustomRegistry::default();
}

pub fn custom_openai_compatible_providers() -> Vec<&'static ProviderDescriptor> {
    lock_read().active.clone()
}

pub fn custom_openai_compatible_provider(name: &str) -> Option<&'static ProviderDescriptor> {
    lock_read()
        .active
        .iter()
        .copied()
        .find(|descriptor| descriptor.name == name)
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
    let leaked_name = Box::leak(name.to_string().into_boxed_str());
    let descriptor = Box::leak(Box::new(ProviderDescriptor {
        id: ProviderId::OpenAiCompatible,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OPENAI_COMPATIBLE_API_BASE,
        },
        name: leaked_name,
        display_name: leaked_name,
        auth_modes: CUSTOM_AUTH,
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: leaked_name,
        // Same Chat Completions effort field as Ollama. Custom names are not in
        // models.dev, so Unknown must still send the selected level.
        catalog_reasoning: CatalogReasoningPolicy::OffAsNone,
        unknown_effort: UnknownEffortPolicy::SendRequested,
        default_model: None,
    }));
    registry.interned.insert(name.to_string(), descriptor);
    descriptor
}

#[cfg(test)]
#[path = "custom_openai_compatible_tests.rs"]
mod tests;
