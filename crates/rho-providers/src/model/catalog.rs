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
    [
        (
            "openai",
            "OpenAI",
            &[("API Key", "openai"), ("OAuth", "openai-codex")][..],
        ),
        ("anthropic", "Anthropic", &[("API Key", "anthropic")][..]),
        ("google", "Google Gemini", &[("API Key", "google")][..]),
        (
            "github-copilot",
            "GitHub Copilot",
            &[("OAuth", "github-copilot")][..],
        ),
        (
            "moonshot",
            "Moonshot AI",
            &[("API Key", "moonshot"), ("OAuth", "kimi-code")][..],
        ),
        (
            "openrouter",
            "OpenRouter",
            &[("API Key", "openrouter"), ("OAuth", "openrouter-oauth")][..],
        ),
        (
            "xai",
            "xAI",
            &[("API Key", "xai"), ("OAuth", "xai-oauth")][..],
        ),
    ]
    .into_iter()
    .map(|(id, prompt, methods)| LoginGroup {
        id: id.into(),
        prompt: prompt.into(),
        methods: methods
            .iter()
            .map(|(prompt, provider)| LoginMethod {
                prompt: (*prompt).into(),
                target: login_target_for_provider(provider)
                    .expect("login group targets must reference registered providers"),
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
        .filter(|provider| provider.auth_kind != ProviderAuthKind::None)
        .map(|provider| LoginTarget {
            provider: provider.name.into(),
            auth: provider.auth.into(),
            label: provider.login_label.into(),
        })
        .collect()
}

pub fn login_target_for_provider(provider: &str) -> Option<LoginTarget> {
    login_targets()
        .into_iter()
        .find(|target| target.provider == provider)
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
    resolve_model_selection_for_provider_from(model_catalog(), provider.trim(), model.trim())
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
    let xai_models = file.xai_models;
    entries.extend(model_entries("xai", "xai-api-key", xai_models.clone()));
    entries.extend(model_entries("xai-oauth", "xai-oauth", xai_models));
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
        .filter(|provider| auths.iter().any(|auth| auth == provider.auth))
    {
        models.extend(cached_provider_entries(provider.name, provider.auth));
    }
    models.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.model.cmp(&right.model))
    });
    models
}

fn cached_provider_entries(provider: &str, auth: &str) -> Vec<ModelCatalogEntry> {
    provider_models::cached_provider_models(provider)
        .into_iter()
        .map(|model| ModelCatalogEntry {
            provider: model.provider,
            display_name: model.display_name,
            model: model.model,
            auth_modes: vec![auth.to_string()],
        })
        .collect()
}

fn provider_default_auth(provider: &str) -> Option<&'static str> {
    provider::provider_descriptor(provider).map(|descriptor| descriptor.auth)
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
) -> ModelSelection {
    ModelSelection {
        provider: provider.to_string(),
        model: model.model.clone(),
        auth: provider_default_auth(provider)
            .unwrap_or("api-key")
            .to_string(),
        from_catalog: true,
    }
}

fn selection_from_entry(entry: &ModelCatalogEntry) -> ModelSelection {
    ModelSelection {
        provider: entry.provider.clone(),
        model: entry.model.clone(),
        auth: entry
            .auth_modes
            .first()
            .map(String::as_str)
            .unwrap_or("api-key")
            .to_string(),
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
        return resolve_model_selection_for_provider_from(catalog, provider.trim(), model.trim());
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
        [entry] => Ok(selection_from_entry(entry)),
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
) -> Result<ModelSelection, ModelSelectionError> {
    if provider.is_empty() || model.is_empty() {
        return Err(ModelSelectionError::Empty);
    }
    if !implemented_providers().contains(&provider) {
        return Err(ModelSelectionError::UnknownProvider {
            provider: provider.to_string(),
        });
    }
    if provider_uses_cached_models(provider) {
        if let Some(entry) = provider_models::cached_provider_model(provider, model) {
            return Ok(selection_from_provider_model(provider, &entry));
        }
        if builtin_default_model(provider).as_deref() == Some(model) {
            return Ok(ModelSelection {
                provider: provider.to_string(),
                model: model.to_string(),
                auth: provider_default_auth(provider)
                    .unwrap_or("api-key")
                    .to_string(),
                from_catalog: true,
            });
        }
        return Err(unavailable_model_error(provider, model));
    }
    catalog
        .iter()
        .find(|entry| entry.provider == provider && entry.model == model)
        .map(selection_from_entry)
        .ok_or_else(|| unavailable_model_error(provider, model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::model::{
        provider_models::{
            replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
            ProviderModel,
        },
        ReasoningCapabilities,
    };

    fn test_catalog() -> Vec<ModelCatalogEntry> {
        vec![
            ModelCatalogEntry {
                provider: "openai".into(),
                model: "shared-model".into(),
                display_name: "shared-model".into(),
                auth_modes: vec!["api-key".into(), "codex".into()],
            },
            ModelCatalogEntry {
                provider: "openai".into(),
                model: "unique-openai".into(),
                display_name: "unique-openai".into(),
                auth_modes: vec!["api-key".into()],
            },
            ModelCatalogEntry {
                provider: "openai".into(),
                model: "shared-model".into(),
                display_name: "shared-model duplicate".into(),
                auth_modes: vec!["api-key".into()],
            },
            ModelCatalogEntry {
                provider: "openai-codex".into(),
                model: "unique-codex".into(),
                display_name: "unique-codex".into(),
                auth_modes: vec!["codex".into()],
            },
            ModelCatalogEntry {
                provider: "anthropic".into(),
                model: "unique-anthropic".into(),
                display_name: "unique-anthropic".into(),
                auth_modes: vec!["anthropic-api-key".into()],
            },
            ModelCatalogEntry {
                provider: "future".into(),
                model: "future-model".into(),
                display_name: "future-model".into(),
                auth_modes: vec!["api-key".into()],
            },
        ]
    }

    fn unique_cache_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("rho-catalog-{name}-{}-{nanos}", std::process::id()))
    }

    fn with_cached_provider_models<T>(
        provider: &str,
        models: Vec<ProviderModel>,
        f: impl FnOnce() -> T,
    ) -> T {
        let cache_dir = unique_cache_dir(provider);
        let result = with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
            replace_cached_provider_models_for_tests(provider, &models).unwrap();
            f()
        });
        let _ = std::fs::remove_dir_all(cache_dir);
        result
    }

    fn provider_model(provider: &str, model: &str) -> ProviderModel {
        ProviderModel {
            provider: provider.into(),
            model: model.into(),
            display_name: model.into(),
            context_window: None,
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        }
    }

    #[test]
    fn parses_embedded_model_catalog() {
        let catalog = model_catalog();

        assert!(catalog
            .iter()
            .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.3-codex-spark"));
        assert!(catalog
            .iter()
            .any(|entry| entry.provider == "xai" && entry.model == "grok-4.5"));
        assert!(catalog
            .iter()
            .any(|entry| entry.provider == "xai" && entry.model == "grok-build-0.1"));
        assert!(catalog
            .iter()
            .any(|entry| entry.provider == "xai" && entry.model == "grok-composer-2.5-fast"));
        assert!(catalog
            .iter()
            .any(|entry| entry.provider == "xai" && entry.model == "grok-4.3"));
        assert!(!catalog
            .iter()
            .any(|entry| entry.provider == "github-copilot"));
    }

    #[test]
    fn available_models_includes_xai_static_catalog() {
        let models = available_models_for_auths(&["xai-oauth".into()]);

        assert!(models.iter().all(|entry| entry.provider == "xai-oauth"));
        assert_eq!(
            models
                .iter()
                .map(|entry| entry.model.as_str())
                .collect::<Vec<_>>(),
            vec![
                "grok-4.3",
                "grok-4.5",
                "grok-build-0.1",
                "grok-composer-2.5-fast",
            ]
        );
        assert_eq!(
            default_model_for_provider("xai").as_deref(),
            Some("grok-4.5")
        );
    }

    #[test]
    fn resolves_xai_static_catalog_selection() {
        let selection = resolve_model_selection_for_auths(
            "xai-oauth/grok-4.5",
            "openai",
            "api-key",
            &["xai-oauth".into()],
        )
        .unwrap();

        assert_eq!(
            selection,
            ModelSelection {
                provider: "xai-oauth".into(),
                model: "grok-4.5".into(),
                auth: "xai-oauth".into(),
                from_catalog: true,
            }
        );
    }

    #[test]
    fn available_models_filters_to_implemented_providers() {
        let models = available_models_for_auths(&["codex".into()]);

        assert!(models.iter().all(|entry| entry.provider == "openai-codex"));
        assert!(models
            .iter()
            .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.5"));
        assert!(models
            .iter()
            .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.4-mini"));
        assert!(models
            .iter()
            .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.3-codex-spark"));
        assert!(models
            .iter()
            .all(|entry| implemented_providers().contains(&entry.provider.as_str())));
    }

    #[test]
    fn available_models_for_auths_uses_static_catalog_for_subscription_models() {
        let models = available_models_for_auths(&["api-key".into(), "codex".into()]);

        assert!(models.iter().any(|entry| entry.provider == "openai-codex"));
    }

    #[test]
    fn login_targets_use_provider_names() {
        let targets = login_targets();

        let providers = targets
            .iter()
            .map(|target| (target.provider.as_str(), target.auth.as_str()))
            .collect::<Vec<_>>();
        assert!(providers.contains(&("openai", "api-key")));
        assert!(providers.contains(&("openai-codex", "codex")));
        assert!(providers.contains(&("anthropic", "anthropic-api-key")));
        assert!(providers.contains(&("google", "google-api-key")));
        assert!(providers.contains(&("github-copilot", "github-copilot")));
        assert!(providers.contains(&("moonshot", "moonshot-api-key")));
        assert!(providers.contains(&("openrouter", "openrouter-api-key")));
        assert!(providers.contains(&("openrouter-oauth", "openrouter-oauth")));
        assert!(providers.contains(&("kimi-code", "kimi-oauth")));
        assert!(providers.contains(&("xai", "xai-api-key")));
        assert!(providers.contains(&("xai-oauth", "xai-oauth")));
        let google = login_group("google").expect("Google login group");
        assert_eq!(google.methods.len(), 1);
        assert_eq!(google.methods[0].target.provider, "google");
        assert!(login_target_for_provider("ollama").is_none());
        assert!(login_target_for_provider("api-key").is_none());
        assert!(login_target_for_provider("codex").is_none());
        assert!(login_target_for_provider("anthropic-api-key").is_none());
        assert!(login_target_for_provider("xai-api-key").is_none());
        assert!(login_target_for_provider("xai-oauth").is_some());
    }

    #[test]
    fn github_copilot_requires_cached_models() {
        let cache_dir = unique_cache_dir("github-copilot-empty");
        with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
            assert_eq!(default_model_for_provider("github-copilot"), None);
            let err = resolve_model_selection_for_auths(
                "github-copilot/gpt-4.1",
                "openai",
                "api-key",
                &["github-copilot".into()],
            )
            .unwrap_err();
            assert_eq!(
                err,
                ModelSelectionError::UnavailableModel {
                    provider: "github-copilot".into(),
                    model: "gpt-4.1".into(),
                    hint: "Open /config and choose Refresh model lists to update available models.",
                }
            );
        });
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn github_copilot_uses_cached_models_in_picker() {
        with_cached_provider_models(
            "github-copilot",
            vec![provider_model("github-copilot", "cached-copilot-model")],
            || {
                let models = available_models_for_auths(&["github-copilot".into()]);
                assert!(models
                    .iter()
                    .any(|entry| entry.model == "cached-copilot-model"));
            },
        );
    }

    #[test]
    fn resolves_provider_model_selection() {
        with_cached_provider_models("openai", vec![provider_model("openai", "gpt-5.5")], || {
            let selection = resolve_model_selection_for_auths(
                "openai/gpt-5.5",
                "openai",
                "codex",
                &["codex".into()],
            )
            .unwrap();

            assert_eq!(
                selection,
                ModelSelection {
                    provider: "openai".into(),
                    model: "gpt-5.5".into(),
                    auth: "api-key".into(),
                    from_catalog: true,
                }
            );
        });
    }

    #[test]
    fn resolves_anthropic_provider_model_selection() {
        with_cached_provider_models(
            "anthropic",
            vec![provider_model("anthropic", "claude-sonnet-4-5")],
            || {
                let selection = resolve_model_selection_for_auths(
                    "anthropic/claude-sonnet-4-5",
                    "openai",
                    "api-key",
                    &["anthropic-api-key".into()],
                )
                .unwrap();

                assert_eq!(
                    selection,
                    ModelSelection {
                        provider: "anthropic".into(),
                        model: "claude-sonnet-4-5".into(),
                        auth: "anthropic-api-key".into(),
                        from_catalog: true,
                    }
                );
            },
        );
    }

    #[test]
    fn resolves_bare_cached_api_model_to_provider() {
        with_cached_provider_models(
            "openai",
            vec![provider_model("openai", "unique-openai")],
            || {
                let catalog = test_catalog();
                let selection = resolve_model_selection_from(
                    &catalog,
                    "unique-openai",
                    "openai",
                    "api-key",
                    &["api-key".into()],
                )
                .unwrap();

                assert_eq!(
                    selection,
                    ModelSelection {
                        provider: "openai".into(),
                        model: "unique-openai".into(),
                        auth: "api-key".into(),
                        from_catalog: true,
                    }
                );
            },
        );
    }

    #[test]
    fn resolves_bare_model_across_all_available_auths() {
        let catalog = test_catalog();
        let selection = resolve_model_selection_from(
            &catalog,
            "unique-codex",
            "openai",
            "api-key",
            &["api-key".into(), "codex".into()],
        )
        .unwrap();

        assert_eq!(
            selection,
            ModelSelection {
                provider: "openai-codex".into(),
                model: "unique-codex".into(),
                auth: "codex".into(),
                from_catalog: true,
            }
        );
    }

    #[test]
    fn resolves_bare_unique_anthropic_model() {
        with_cached_provider_models(
            "anthropic",
            vec![provider_model("anthropic", "unique-anthropic")],
            || {
                let catalog = test_catalog();
                let selection = resolve_model_selection_from(
                    &catalog,
                    "unique-anthropic",
                    "openai",
                    "api-key",
                    &["api-key".into(), "anthropic-api-key".into()],
                )
                .unwrap();

                assert_eq!(
                    selection,
                    ModelSelection {
                        provider: "anthropic".into(),
                        model: "unique-anthropic".into(),
                        auth: "anthropic-api-key".into(),
                        from_catalog: true,
                    }
                );
            },
        );
    }

    #[test]
    fn resolves_bare_unique_codex_model() {
        let selection = resolve_model_selection_for_auths(
            "gpt-5.3-codex-spark",
            "openai-codex",
            "codex",
            &["codex".into()],
        )
        .unwrap();

        assert_eq!(
            selection,
            ModelSelection {
                provider: "openai-codex".into(),
                model: "gpt-5.3-codex-spark".into(),
                auth: "codex".into(),
                from_catalog: true,
            }
        );
    }

    #[test]
    fn anthropic_uncached_provider_model_is_rejected() {
        let err = resolve_model_selection_for_auths(
            "anthropic/custom-model",
            "openai",
            "api-key",
            &["api-key".into()],
        )
        .unwrap_err();

        assert_eq!(
            err,
            ModelSelectionError::UnavailableModel {
                provider: "anthropic".into(),
                model: "custom-model".into(),
                hint: "Open /config and choose Refresh model lists to update available models.",
            }
        );
    }

    #[test]
    fn bare_uncached_current_provider_model_is_rejected() {
        let err = resolve_model_selection_for_auths(
            "brand-new-model",
            "openai",
            "codex",
            &["codex".into()],
        )
        .unwrap_err();

        assert_eq!(
            err,
            ModelSelectionError::UnavailableModel {
                provider: "openai".into(),
                model: "brand-new-model".into(),
                hint: "Open /config and choose Refresh model lists to update available models.",
            }
        );
    }

    #[test]
    fn bare_ambiguous_model_returns_error() {
        with_cached_provider_models(
            "openai",
            vec![provider_model("openai", "shared-model")],
            || {
                let catalog = vec![ModelCatalogEntry {
                    provider: "openai-codex".into(),
                    model: "shared-model".into(),
                    display_name: "shared-model".into(),
                    auth_modes: vec!["codex".into()],
                }];
                let err = resolve_model_selection_from(
                    &catalog,
                    "shared-model",
                    "openai",
                    "api-key",
                    &["api-key".into(), "codex".into()],
                )
                .unwrap_err();

                assert_eq!(
                    err,
                    ModelSelectionError::AmbiguousModel {
                        model: "shared-model".into()
                    }
                );
            },
        );
    }

    #[test]
    fn non_allowlisted_codex_model_is_rejected() {
        let err = resolve_model_selection_for_auths(
            "openai-codex/custom-model",
            "openai-codex",
            "codex",
            &["codex".into()],
        )
        .unwrap_err();

        assert_eq!(
            err,
            ModelSelectionError::UnavailableModel {
                provider: "openai-codex".into(),
                model: "custom-model".into(),
                hint: "Choose a model from the provider allowlist.",
            }
        );
    }

    #[test]
    fn unknown_provider_is_rejected() {
        let err = resolve_model_selection_for_auths(
            "missing/gpt-5.5",
            "openai",
            "codex",
            &["codex".into()],
        )
        .unwrap_err();

        assert_eq!(
            err,
            ModelSelectionError::UnknownProvider {
                provider: "missing".into()
            }
        );
    }
}
