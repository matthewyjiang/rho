use std::sync::OnceLock;

use serde::Deserialize;

use crate::{
    model::provider_models,
    provider::{self, ProviderAuthKind, ProviderModelSource},
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
    provider::providers()
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

pub fn login_groups() -> Vec<LoginGroup> {
    // Keep alphabetical by display `prompt` so login pickers stay sorted as groups are added.
    // Method values are auth profile ids; the resolved LoginTarget carries the provider name.
    [
        (
            "anthropic",
            "Anthropic",
            &[("API Key", "anthropic-api-key")][..],
        ),
        (
            "github-copilot",
            "GitHub Copilot",
            &[("OAuth", "github-copilot")][..],
        ),
        (
            "google",
            "Google Gemini",
            &[("API Key", "google-api-key")][..],
        ),
        (
            "moonshot",
            "Moonshot AI",
            &[("API Key", "moonshot-api-key"), ("OAuth", "kimi-oauth")][..],
        ),
        (
            "ollama-cloud",
            "Ollama Cloud",
            &[
                ("API Key", "ollama-cloud-api-key"),
                ("Device Key", "ollama-cloud-device"),
            ][..],
        ),
        (
            "openai",
            "OpenAI",
            &[("API Key", "api-key"), ("OAuth", "codex")][..],
        ),
        (
            "openrouter",
            "OpenRouter",
            &[
                ("API Key", "openrouter-api-key"),
                ("OAuth", "openrouter-oauth"),
            ][..],
        ),
        (
            "poolside",
            "Poolside",
            &[("API Key", "poolside-api-key")][..],
        ),
        (
            "xai",
            "xAI",
            &[("API Key", "xai-api-key"), ("OAuth", "xai-oauth")][..],
        ),
    ]
    .into_iter()
    .map(|(id, prompt, methods)| LoginGroup {
        id: id.into(),
        prompt: prompt.into(),
        methods: methods
            .iter()
            .map(|(prompt, auth)| LoginMethod {
                prompt: (*prompt).into(),
                target: login_target_for_auth(auth)
                    .expect("login group targets must reference registered auth profiles"),
            })
            .collect(),
    })
    .collect()
}

pub fn login_group(id: &str) -> Option<LoginGroup> {
    login_groups().into_iter().find(|group| group.id == id)
}

pub fn login_targets() -> Vec<LoginTarget> {
    provider::providers()
        .iter()
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
    match provider::provider_descriptor(provider)?.model_source {
        ProviderModelSource::CachedProviderModels => {
            provider_models::cached_provider_models(provider)
                .into_iter()
                .next()
                .map(|entry| entry.model)
                .or_else(|| builtin_default_model(provider))
        }
        ProviderModelSource::StaticCatalog => static_catalog_default_model(provider),
    }
}

fn static_catalog_default_model(provider: &str) -> Option<String> {
    model_catalog()
        .iter()
        .find(|entry| entry.provider == provider)
        .map(|entry| entry.model.clone())
}

fn builtin_default_model(provider: &str) -> Option<String> {
    match provider {
        "anthropic" => Some("claude-sonnet-4-5".into()),
        "google" => Some("gemini-3.1-flash-lite".into()),
        _ => None,
    }
}

pub fn resolve_model_selection_for_provider(
    provider: &str,
    model: &str,
) -> Result<ModelSelection, ModelSelectionError> {
    resolve_model_selection_for_provider_from(model_catalog(), provider.trim(), model.trim(), None)
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
    for provider in provider::providers()
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

fn provider_default_auth(provider: &str) -> Option<&'static str> {
    provider::provider_descriptor(provider).map(|descriptor| descriptor.default_auth().id)
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

fn selection_from_provider_model(
    provider: &str,
    model: &provider_models::ProviderModel,
    preferred_auth: Option<&str>,
) -> ModelSelection {
    let auth = preferred_auth
        .and_then(|auth| {
            provider::provider_descriptor(provider)
                .and_then(|descriptor| descriptor.auth_mode(auth).map(|mode| mode.id))
        })
        .or_else(|| provider_default_auth(provider))
        .unwrap_or("api-key");
    ModelSelection {
        provider: provider.to_string(),
        model: model.model.clone(),
        auth: auth.to_string(),
        from_catalog: true,
    }
}

fn selection_from_entry(entry: &ModelCatalogEntry, preferred_auth: Option<&str>) -> ModelSelection {
    let auth = preferred_auth
        .filter(|auth| entry.auth_modes.iter().any(|mode| mode == auth))
        .map(str::to_string)
        .or_else(|| entry.auth_modes.first().cloned())
        .unwrap_or_else(|| "api-key".into());
    ModelSelection {
        provider: entry.provider.clone(),
        model: entry.model.clone(),
        auth,
        from_catalog: true,
    }
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

    if let Some((provider, model)) = input.split_once('/') {
        return resolve_model_selection_for_provider_from(
            catalog,
            provider.trim(),
            model.trim(),
            Some(auth),
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
        [entry] => Ok(selection_from_entry(entry, Some(auth))),
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
    preferred_auth: Option<&str>,
) -> Result<ModelSelection, ModelSelectionError> {
    if provider.is_empty() || model.is_empty() {
        return Err(ModelSelectionError::Empty);
    }
    let (provider, alias_auth) = provider::legacy_provider_alias(provider)
        .map(|(provider, auth)| (provider, Some(auth)))
        .unwrap_or((provider, None));
    let preferred_auth = alias_auth.or(preferred_auth);
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
        if let Some(entry) = provider_models::cached_provider_model(provider, &model_id) {
            return Ok(selection_from_provider_model(
                provider,
                &entry,
                preferred_auth,
            ));
        }
        if builtin_default_model(provider).as_deref() == Some(model_id.as_str()) {
            let auth = preferred_auth
                .and_then(|auth| {
                    provider::provider_descriptor(provider)
                        .and_then(|descriptor| descriptor.auth_mode(auth).map(|mode| mode.id))
                })
                .or_else(|| provider_default_auth(provider))
                .unwrap_or("api-key");
            return Ok(ModelSelection {
                provider: provider.to_string(),
                model: model_id,
                auth: auth.to_string(),
                from_catalog: true,
            });
        }
        return Err(unavailable_model_error(provider, model));
    }
    catalog
        .iter()
        .find(|entry| entry.provider == provider && entry.model == model)
        .map(|entry| selection_from_entry(entry, preferred_auth))
        .ok_or_else(|| unavailable_model_error(provider, model))
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
