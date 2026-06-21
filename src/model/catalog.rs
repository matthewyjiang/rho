#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelCatalogEntry {
    pub provider: &'static str,
    pub model: &'static str,
    pub display_name: &'static str,
    pub auth_modes: &'static [&'static str],
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

pub const MODEL_CATALOG: &[ModelCatalogEntry] = &[
    ModelCatalogEntry {
        provider: "openai",
        model: "gpt-5.5",
        display_name: "gpt-5.5",
        auth_modes: &["api-key"],
    },
    ModelCatalogEntry {
        provider: "openai-codex",
        model: "gpt-5.5",
        display_name: "gpt-5.5",
        auth_modes: &["codex"],
    },
    ModelCatalogEntry {
        provider: "openai-codex",
        model: "gpt-5.4-mini",
        display_name: "gpt-5.4-mini",
        auth_modes: &["codex"],
    },
    ModelCatalogEntry {
        provider: "openai-codex",
        model: "gpt-5.3-codex-spark",
        display_name: "gpt-5.3-codex-spark",
        auth_modes: &["codex"],
    },
];

const IMPLEMENTED_PROVIDERS: &[&str] = &["openai", "openai-codex"];

pub fn implemented_providers() -> &'static [&'static str] {
    IMPLEMENTED_PROVIDERS
}

pub fn available_models(auth: &str) -> Vec<ModelCatalogEntry> {
    available_models_from(MODEL_CATALOG, auth)
}

pub fn resolve_model_selection(
    input: &str,
    current_provider: &str,
    auth: &str,
) -> Result<ModelSelection, ModelSelectionError> {
    resolve_model_selection_from(MODEL_CATALOG, input, current_provider, auth)
}

fn available_models_from(catalog: &[ModelCatalogEntry], _auth: &str) -> Vec<ModelCatalogEntry> {
    let mut models = catalog
        .iter()
        .copied()
        .filter(|entry| implemented_providers().contains(&entry.provider))
        .collect::<Vec<_>>();
    models.sort_by(|left, right| {
        left.provider
            .cmp(right.provider)
            .then_with(|| left.model.cmp(right.model))
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
        provider: entry.provider.to_string(),
        model: entry.model.to_string(),
        auth: entry
            .auth_modes
            .first()
            .copied()
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

    const TEST_CATALOG: &[ModelCatalogEntry] = &[
        ModelCatalogEntry {
            provider: "openai",
            model: "shared-model",
            display_name: "shared-model",
            auth_modes: &["api-key", "codex"],
        },
        ModelCatalogEntry {
            provider: "openai",
            model: "unique-openai",
            display_name: "unique-openai",
            auth_modes: &["api-key"],
        },
        ModelCatalogEntry {
            provider: "openai",
            model: "shared-model",
            display_name: "shared-model duplicate",
            auth_modes: &["api-key"],
        },
        ModelCatalogEntry {
            provider: "future",
            model: "future-model",
            display_name: "future-model",
            auth_modes: &["api-key"],
        },
    ];

    #[test]
    fn available_models_filters_to_implemented_providers() {
        let models = available_models("codex");

        assert!(models
            .iter()
            .any(|entry| entry.provider == "openai" && entry.model == "gpt-5.5"));
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
            .all(|entry| implemented_providers().contains(&entry.provider)));
    }

    #[test]
    fn resolves_provider_model_selection() {
        let selection = resolve_model_selection("openai/gpt-5.5", "openai", "codex").unwrap();

        assert_eq!(selection.provider, "openai");
        assert_eq!(selection.model, "gpt-5.5");
        assert_eq!(selection.auth, "api-key");
        assert!(selection.from_catalog);
    }

    #[test]
    fn resolves_bare_unique_model_to_catalog_provider() {
        let selection =
            resolve_model_selection_from(TEST_CATALOG, "unique-openai", "openai", "api-key")
                .unwrap();

        assert_eq!(selection.provider, "openai");
        assert_eq!(selection.model, "unique-openai");
        assert_eq!(selection.auth, "api-key");
        assert!(selection.from_catalog);
    }

    #[test]
    fn resolves_bare_unique_codex_model() {
        let selection = resolve_model_selection("gpt-5.4-mini", "openai", "api-key").unwrap();

        assert_eq!(selection.provider, "openai-codex");
        assert_eq!(selection.model, "gpt-5.4-mini");
        assert_eq!(selection.auth, "codex");
        assert!(selection.from_catalog);
    }

    #[test]
    fn bare_uncataloged_model_uses_current_provider() {
        let selection = resolve_model_selection("brand-new-model", "openai", "codex").unwrap();

        assert_eq!(selection.provider, "openai");
        assert_eq!(selection.model, "brand-new-model");
        assert_eq!(selection.auth, "api-key");
        assert!(!selection.from_catalog);
    }

    #[test]
    fn bare_ambiguous_model_returns_error() {
        let err = resolve_model_selection_from(TEST_CATALOG, "shared-model", "openai", "api-key")
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
