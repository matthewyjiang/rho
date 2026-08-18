use std::sync::OnceLock;

use serde::Deserialize;

use crate::{
    model::provider_models,
    provider::{self, ProviderAuthKind, ProviderModelSource, KEYLESS_AUTH},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelCatalogEntry {
    pub provider: String,
    pub model: String,
    pub display_name: String,
    pub auth_modes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginGroup {
    pub id: String,
    pub prompt: String,
    pub methods: Vec<LoginMethod>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginMethod {
    pub prompt: String,
    pub target: LoginTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginTarget {
    pub provider: String,
    pub auth: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
    pub auth: String,
    pub from_catalog: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModelSelectionError {
    #[error("unknown provider '{provider}' for model selection")]
    UnknownProvider { provider: String },
    #[error("model '{model}' is available from multiple providers; use /model provider/model")]
    AmbiguousModel { model: String },
    #[error("model selection cannot be empty")]
    Empty,
    #[error("model '{model}' is not available for provider '{provider}'. {hint}")]
    UnavailableModel {
        provider: String,
        model: String,
        hint: &'static str,
    },
}

#[derive(Deserialize)]
struct ModelCatalogFile {
    openai_codex_models: Vec<String>,
    xai_models: Vec<String>,
}

const MODEL_CATALOG_TOML: &str = include_str!("models.toml");
static MODEL_CATALOG: OnceLock<Vec<ModelCatalogEntry>> = OnceLock::new();

pub fn implemented_providers() -> Vec<&'static str> {
    provider::visible_providers()
        .iter()
        .map(|provider| provider.name)
        .collect()
}

pub fn model_catalog() -> &'static [ModelCatalogEntry] {
    MODEL_CATALOG.get_or_init(|| parse_model_catalog(MODEL_CATALOG_TOML))
}

pub fn available_models_for_auths(auths: &[String]) -> Vec<ModelCatalogEntry> {
    available_models_for_auths_from(model_catalog(), auths)
}

struct CrossProviderLoginGroup {
    id: &'static str,
    prompt: &'static str,
    auths: &'static [&'static str],
}

/// Cross-provider login groups that attach foreign auth modes under one picker entry.
///
/// Single-provider groups are derived from [`provider::visible_providers`]; only groupings that
/// span providers (OpenAI+Codex, Moonshot+Kimi) need an explicit spec.
///
/// Auth prompts are derived from registry metadata so `ApiKey`/`OAuth` wording stays
/// consistent with the provider descriptor. Each group lists auth profile ids; the
/// prompt comes from the descriptor at build time.
const CROSS_PROVIDER_LOGIN_GROUPS: &[CrossProviderLoginGroup] = &[
    CrossProviderLoginGroup {
        id: "openai",
        prompt: "OpenAI",
        auths: &["api-key", "codex"],
    },
    CrossProviderLoginGroup {
        id: "moonshot",
        prompt: "Moonshot AI",
        auths: &["moonshot-api-key", "kimi-oauth"],
    },
];

pub fn login_groups() -> Vec<LoginGroup> {
    let mut claimed_auths = std::collections::BTreeSet::<&'static str>::new();
    let mut groups = Vec::new();

    for group in CROSS_PROVIDER_LOGIN_GROUPS {
        let methods = group
            .auths
            .iter()
            .map(|auth| {
                claimed_auths.insert(*auth);
                let (descriptor, mode) = provider::resolve_auth_mode(auth)
                    .expect("login group targets must reference registered auth profiles");
                LoginMethod {
                    prompt: login_method_prompt(mode.auth_kind).to_string(),
                    target: LoginTarget {
                        provider: descriptor.name.into(),
                        auth: mode.id.into(),
                        label: mode.login_label.into(),
                    },
                }
            })
            .collect::<Vec<_>>();
        groups.push(LoginGroup {
            id: group.id.into(),
            prompt: group.prompt.into(),
            methods,
        });
    }

    for descriptor in provider::visible_providers() {
        let modes = descriptor
            .auth_modes()
            .filter(|mode| mode.auth_kind != ProviderAuthKind::None)
            .filter(|mode| !claimed_auths.contains(mode.id))
            .collect::<Vec<_>>();
        if modes.is_empty() {
            continue;
        }
        for mode in &modes {
            claimed_auths.insert(mode.id);
        }
        groups.push(LoginGroup {
            id: descriptor.name.into(),
            prompt: descriptor.display_name.into(),
            methods: modes
                .into_iter()
                .map(|mode| LoginMethod {
                    prompt: login_method_prompt(mode.auth_kind).into(),
                    target: LoginTarget {
                        provider: descriptor.name.into(),
                        auth: mode.id.into(),
                        label: mode.login_label.into(),
                    },
                })
                .collect(),
        });
    }

    groups.sort_by(|left, right| left.prompt.cmp(&right.prompt));
    groups
}

fn login_method_prompt(auth_kind: ProviderAuthKind) -> &'static str {
    match auth_kind {
        ProviderAuthKind::None => "None",
        ProviderAuthKind::ApiKey { .. } => "API Key",
        ProviderAuthKind::OllamaDeviceKey { .. } => "Device Key",
        ProviderAuthKind::CodexOAuth { .. }
        | ProviderAuthKind::GithubCopilotDevice { .. }
        | ProviderAuthKind::XaiOAuth { .. }
        | ProviderAuthKind::BearerCredential { .. }
        | ProviderAuthKind::KimiOAuth { .. } => "OAuth",
    }
}

pub fn login_group(id: &str) -> Option<LoginGroup> {
    login_groups().into_iter().find(|group| group.id == id)
}

pub fn login_targets() -> Vec<LoginTarget> {
    provider::visible_providers()
        .into_iter()
        .flat_map(|provider| {
            provider
                .auth_modes()
                .filter(|mode| mode.auth_kind != ProviderAuthKind::None)
                .map(|mode| LoginTarget {
                    provider: provider.name.into(),
                    auth: mode.id.into(),
                    label: mode.login_label.into(),
                })
        })
        .collect()
}

pub fn login_target_for_auth(auth: &str) -> Option<LoginTarget> {
    login_targets()
        .into_iter()
        .find(|target| target.auth == auth)
}

pub fn login_target_for_provider(provider: &str) -> Option<LoginTarget> {
    // Prefer an exact auth profile id, then a unique provider with one login mode.
    if let Some(target) = login_target_for_auth(provider) {
        return Some(target);
    }
    let matches = login_targets()
        .into_iter()
        .filter(|target| target.provider == provider)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

pub fn default_model_for_provider(provider: &str) -> Option<String> {
    let descriptor = provider::provider_descriptor(provider)?;
    match descriptor.model_source {
        ProviderModelSource::CachedProviderModels => {
            let cached = provider_models::cached_provider_models(provider);
            preferred_cached_default(descriptor.default_model, &cached)
        }
        ProviderModelSource::StaticCatalog => static_catalog_default_model(provider),
    }
}

/// Prefer the descriptor default when present in cache; else first cached; else bare default.
///
/// Cache order is lexicographic by model id, so "first cached" is not a product preference.
fn preferred_cached_default(
    default_model: Option<&str>,
    cached: &[provider_models::ProviderModel],
) -> Option<String> {
    if let Some(preferred) = default_model {
        if let Some(found) = cached.iter().find(|entry| entry.model == preferred) {
            return Some(found.model.clone());
        }
    }
    if let Some(first) = cached.first() {
        return Some(first.model.clone());
    }
    default_model.map(str::to_string)
}

fn static_catalog_default_model(provider: &str) -> Option<String> {
    model_catalog()
        .iter()
        .find(|entry| entry.provider == provider)
        .map(|entry| entry.model.clone())
}

fn descriptor_default_model(provider: &str) -> Option<&'static str> {
    provider::provider_descriptor(provider).and_then(|descriptor| descriptor.default_model)
}

/// Auth context a caller resolves model selections against.
///
/// Selection prefers `current` when the target provider offers it, then a
/// stored key over a keyless mode, then the first offered mode in
/// `available`, then the provider default.
#[derive(Clone, Copy)]
pub struct SelectionAuthContext<'a> {
    /// The auth mode active before the selection, if any.
    pub current: Option<&'a str>,
    /// Auth modes with stored credentials.
    pub available: &'a [String],
}

impl SelectionAuthContext<'_> {
    /// No credential store in scope; selection keeps the provider default auth.
    pub fn none() -> SelectionAuthContext<'static> {
        SelectionAuthContext {
            current: None,
            available: &[],
        }
    }

    /// Selects the auth mode for a model selection from the given candidate
    /// modes: current auth first, then a stored key over [`crate::provider::KEYLESS_AUTH`],
    /// then the first candidate so callers without credential context keep the
    /// provider default.
    pub fn select(&self, auth_modes: &[String]) -> String {
        self.current
            .filter(|auth| auth_modes.iter().any(|mode| mode == auth))
            .map(str::to_string)
            .or_else(|| self.preferred_available(auth_modes))
            .or_else(|| auth_modes.first().cloned())
            .unwrap_or_else(|| "api-key".into())
    }

    fn preferred_available(&self, auth_modes: &[String]) -> Option<String> {
        let matches: Vec<&String> = auth_modes
            .iter()
            .filter(|mode| self.available.contains(mode))
            .collect();
        matches
            .iter()
            .find(|mode| mode.as_str() != KEYLESS_AUTH)
            .or_else(|| matches.first())
            .map(|mode| (*mode).clone())
    }
}

pub fn resolve_model_selection_for_provider(
    provider: &str,
    model: &str,
    auth_context: SelectionAuthContext<'_>,
) -> Result<ModelSelection, ModelSelectionError> {
    resolve_model_selection_for_provider_from(
        model_catalog(),
        provider.trim(),
        model.trim(),
        auth_context,
    )
}

pub fn resolve_model_selection_for_auths(
    input: &str,
    current_provider: &str,
    auth: &str,
    available_auths: &[String],
) -> Result<ModelSelection, ModelSelectionError> {
    resolve_model_selection_from(
        model_catalog(),
        input,
        current_provider,
        auth,
        available_auths,
    )
}

fn parse_model_catalog(text: &str) -> Vec<ModelCatalogEntry> {
    let file: ModelCatalogFile =
        toml::from_str(text).expect("embedded model catalog must be valid");
    let mut entries = model_entries("openai-codex", "codex", file.openai_codex_models);
    let mut xai_models = model_entries("xai", "xai-api-key", file.xai_models);
    for entry in &mut xai_models {
        entry.auth_modes.push("xai-oauth".into());
    }
    entries.extend(xai_models);
    entries
}

fn model_entries(provider: &str, auth: &str, models: Vec<String>) -> Vec<ModelCatalogEntry> {
    models
        .into_iter()
        .map(|model| ModelCatalogEntry {
            provider: provider.to_string(),
            display_name: model.clone(),
            model,
            auth_modes: vec![auth.to_string()],
        })
        .collect()
}

fn available_models_for_auths_from(
    catalog: &[ModelCatalogEntry],
    auths: &[String],
) -> Vec<ModelCatalogEntry> {
    let mut models = catalog
        .iter()
        .filter(|entry| implemented_providers().contains(&entry.provider.as_str()))
        .filter(|entry| provider_uses_static_catalog(&entry.provider))
        .filter(|entry| {
            entry
                .auth_modes
                .iter()
                .any(|mode| auths.iter().any(|auth| auth == mode))
        })
        .cloned()
        .collect::<Vec<_>>();
    for provider in provider::visible_providers()
        .iter()
        .filter(|provider| provider_uses_cached_models(provider.name))
    {
        let available_modes = provider
            .auth_modes()
            .filter(|mode| auths.iter().any(|auth| auth == mode.id))
            .map(|mode| mode.id.to_string())
            .collect::<Vec<_>>();
        if available_modes.is_empty() {
            continue;
        }
        models.extend(cached_provider_entries(provider.name, &available_modes));
    }
    models.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.model.cmp(&right.model))
    });
    models
}

fn cached_provider_entries(provider: &str, auth_modes: &[String]) -> Vec<ModelCatalogEntry> {
    provider_models::cached_provider_models(provider)
        .into_iter()
        .map(|model| ModelCatalogEntry {
            provider: model.provider,
            display_name: model.display_name,
            model: model.model,
            auth_modes: auth_modes.to_vec(),
        })
        .collect()
}

fn provider_uses_cached_models(provider: &str) -> bool {
    provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.model_source == ProviderModelSource::CachedProviderModels)
        .unwrap_or(false)
}

fn provider_uses_static_catalog(provider: &str) -> bool {
    provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.model_source == ProviderModelSource::StaticCatalog)
        .unwrap_or(false)
}

fn unavailable_model_error(provider: &str, model: &str) -> ModelSelectionError {
    let hint = if provider_uses_cached_models(provider) {
        "Open /config and choose Refresh model lists to update available models."
    } else {
        "Choose a model from the provider allowlist."
    };
    ModelSelectionError::UnavailableModel {
        provider: provider.to_string(),
        model: model.to_string(),
        hint,
    }
}

fn selection_from_entry(
    entry: &ModelCatalogEntry,
    auth_context: SelectionAuthContext<'_>,
) -> ModelSelection {
    ModelSelection {
        provider: entry.provider.clone(),
        model: entry.model.clone(),
        auth: auth_context.select(&entry.auth_modes),
        from_catalog: true,
    }
}

fn provider_auth_mode_ids(provider: &str) -> Vec<String> {
    provider::provider_descriptor(provider)
        .map(|descriptor| {
            descriptor
                .auth_modes()
                .map(|mode| mode.id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_model_selection_from(
    catalog: &[ModelCatalogEntry],
    input: &str,
    current_provider: &str,
    auth: &str,
    available_auths: &[String],
) -> Result<ModelSelection, ModelSelectionError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ModelSelectionError::Empty);
    }

    let auth_context = SelectionAuthContext {
        current: Some(auth),
        available: available_auths,
    };
    if let Some((provider, model)) = input.split_once('/') {
        return resolve_model_selection_for_provider_from(
            catalog,
            provider.trim(),
            model.trim(),
            auth_context,
        );
    }

    let auths = if available_auths.is_empty() {
        vec![auth.to_string()]
    } else {
        available_auths.to_vec()
    };
    let matches = available_models_for_auths_from(catalog, &auths)
        .into_iter()
        .filter(|entry| entry.model == input)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [entry] => Ok(selection_from_entry(entry, auth_context)),
        [] => Err(unavailable_model_error(current_provider, input)),
        _ => Err(ModelSelectionError::AmbiguousModel {
            model: input.to_string(),
        }),
    }
}

fn resolve_model_selection_for_provider_from(
    catalog: &[ModelCatalogEntry],
    provider: &str,
    model: &str,
    auth_context: SelectionAuthContext<'_>,
) -> Result<ModelSelection, ModelSelectionError> {
    if provider.is_empty() || model.is_empty() {
        return Err(ModelSelectionError::Empty);
    }
    let (provider, alias_auth) = provider::legacy_provider_alias(provider)
        .map(|(provider, auth)| (provider, Some(auth)))
        .unwrap_or((provider, None));
    // A legacy provider alias names its auth mode explicitly, so it overrides
    // the caller's current auth.
    let auth_context = SelectionAuthContext {
        current: alias_auth.or(auth_context.current),
        ..auth_context
    };
    if !implemented_providers().contains(&provider) {
        return Err(ModelSelectionError::UnknownProvider {
            provider: provider.to_string(),
        });
    }
    if provider_uses_cached_models(provider) {
        let model_id = provider::provider_descriptor(provider).map_or_else(
            || model.to_string(),
            |descriptor| descriptor.canonicalize_model_id(model),
        );
        let selected_model = provider_models::cached_provider_model(provider, &model_id)
            .map(|entry| entry.model)
            .or_else(|| {
                (descriptor_default_model(provider) == Some(model_id.as_str())).then_some(model_id)
            });
        let Some(selected_model) = selected_model else {
            return Err(unavailable_model_error(provider, model));
        };
        return Ok(ModelSelection {
            provider: provider.to_string(),
            model: selected_model,
            auth: auth_context.select(&provider_auth_mode_ids(provider)),
            from_catalog: true,
        });
    }
    catalog
        .iter()
        .find(|entry| entry.provider == provider && entry.model == model)
        .map(|entry| selection_from_entry(entry, auth_context))
        .ok_or_else(|| unavailable_model_error(provider, model))
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
