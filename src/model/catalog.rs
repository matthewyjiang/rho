use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelCatalogEntry {
    pub provider: String,
    pub model: String,
    pub display_name: String,
    pub auth_modes: Vec<String>,
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
}

#[derive(Deserialize)]
struct ModelCatalogFile {
    openai_api_models: Vec<String>,
    openai_codex_models: Vec<String>,
}

const MODEL_CATALOG_TOML: &str = include_str!("models.toml");
const IMPLEMENTED_PROVIDERS: &[&str] = &["openai", "openai-codex"];

static MODEL_CATALOG: OnceLock<Vec<ModelCatalogEntry>> = OnceLock::new();

pub fn implemented_providers() -> &'static [&'static str] {
    IMPLEMENTED_PROVIDERS
}

pub fn model_catalog() -> &'static [ModelCatalogEntry] {
    MODEL_CATALOG.get_or_init(|| parse_model_catalog(MODEL_CATALOG_TOML))
}

pub fn available_models(auth: &str) -> Vec<ModelCatalogEntry> {
    available_models_from(model_catalog(), auth)
}

pub fn resolve_model_selection(
    input: &str,
    current_provider: &str,
    auth: &str,
) -> Result<ModelSelection, ModelSelectionError> {
    resolve_model_selection_from(model_catalog(), input, current_provider, auth)
}

fn parse_model_catalog(text: &str) -> Vec<ModelCatalogEntry> {
    let file: ModelCatalogFile =
        toml::from_str(text).expect("embedded model catalog must be valid");
    let mut entries = Vec::new();
    entries.extend(model_entries("openai", "api-key", file.openai_api_models));
    entries.extend(model_entries(
        "openai-codex",
        "codex",
        file.openai_codex_models,
    ));
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

fn available_models_from(catalog: &[ModelCatalogEntry], auth: &str) -> Vec<ModelCatalogEntry> {
    let mut models = catalog
        .iter()
        .filter(|entry| implemented_providers().contains(&entry.provider.as_str()))
        .filter(|entry| entry.auth_modes.iter().any(|mode| mode == auth))
        .cloned()
        .collect::<Vec<_>>();
    models.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.model.cmp(&right.model))
    });
    models
}

fn provider_default_auth(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("api-key"),
        "openai-codex" => Some("codex"),
        _ => None,
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
) -> Result<ModelSelection, ModelSelectionError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ModelSelectionError::Empty);
    }

    if let Some((provider, model)) = input.split_once('/') {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() {
            return Err(ModelSelectionError::Empty);
        }
        if !IMPLEMENTED_PROVIDERS.contains(&provider) {
            return Err(ModelSelectionError::UnknownProvider {
                provider: provider.to_string(),
            });
        }
        if let Some(entry) = catalog
            .iter()
            .find(|entry| entry.provider == provider && entry.model == model)
        {
            return Ok(selection_from_entry(entry));
        }
        return Ok(ModelSelection {
            provider: provider.to_string(),
            model: model.to_string(),
            auth: provider_default_auth(provider).unwrap_or(auth).to_string(),
            from_catalog: false,
        });
    }

    let matches = available_models_from(catalog, auth)
        .into_iter()
        .filter(|entry| entry.model == input)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [entry] => Ok(selection_from_entry(entry)),
        [] => Ok(ModelSelection {
            provider: current_provider.to_string(),
            model: input.to_string(),
            auth: provider_default_auth(current_provider)
                .unwrap_or(auth)
                .to_string(),
            from_catalog: false,
        }),
        _ => Err(ModelSelectionError::AmbiguousModel {
            model: input.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                provider: "future".into(),
                model: "future-model".into(),
                display_name: "future-model".into(),
                auth_modes: vec!["api-key".into()],
            },
        ]
    }

    #[test]
    fn parses_embedded_model_catalog() {
        let catalog = model_catalog();

        assert!(catalog
            .iter()
            .any(|entry| entry.provider == "openai" && entry.model == "gpt-5.5-pro"));
        assert!(catalog
            .iter()
            .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.3-codex-spark"));
    }

    #[test]
    fn available_models_filters_to_implemented_providers() {
        let models = available_models("codex");

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
    fn resolves_provider_model_selection() {
        let selection = resolve_model_selection("openai/gpt-5.5", "openai", "codex").unwrap();

        assert_eq!(
            selection,
            ModelSelection {
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                auth: "api-key".into(),
                from_catalog: true,
            }
        );
    }

    #[test]
    fn resolves_bare_unique_model_to_catalog_provider() {
        let catalog = test_catalog();
        let selection =
            resolve_model_selection_from(&catalog, "unique-openai", "openai", "api-key").unwrap();

        assert_eq!(
            selection,
            ModelSelection {
                provider: "openai".into(),
                model: "unique-openai".into(),
                auth: "api-key".into(),
                from_catalog: true,
            }
        );
    }

    #[test]
    fn resolves_bare_unique_codex_model() {
        let selection =
            resolve_model_selection("gpt-5.3-codex-spark", "openai-codex", "codex").unwrap();

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
    fn bare_uncataloged_model_uses_current_provider() {
        let selection = resolve_model_selection("brand-new-model", "openai", "codex").unwrap();

        assert_eq!(
            selection,
            ModelSelection {
                provider: "openai".into(),
                model: "brand-new-model".into(),
                auth: "api-key".into(),
                from_catalog: false,
            }
        );
    }

    #[test]
    fn bare_ambiguous_model_returns_error() {
        let catalog = test_catalog();
        let err = resolve_model_selection_from(&catalog, "shared-model", "openai", "api-key")
            .unwrap_err();

        assert_eq!(
            err,
            ModelSelectionError::AmbiguousModel {
                model: "shared-model".into()
            }
        );
    }

    #[test]
    fn unknown_provider_is_rejected() {
        let err = resolve_model_selection("missing/gpt-5.5", "openai", "codex").unwrap_err();

        assert_eq!(
            err,
            ModelSelectionError::UnknownProvider {
                provider: "missing".into()
            }
        );
    }
}
