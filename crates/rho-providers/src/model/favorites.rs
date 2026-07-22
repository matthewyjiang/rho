use crate::model::catalog::ModelCatalogEntry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoriteModel {
    pub provider: String,
    pub model: String,
}

impl FavoriteModel {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        let provider = provider.into();
        let model = model.into();
        let model = crate::provider::provider_descriptor(&provider)
            .map(|descriptor| descriptor.canonicalize_model_id(&model))
            .unwrap_or(model);
        Self { provider, model }
    }

    pub fn value(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }

    pub fn matches(&self, provider: &str, model: &str) -> bool {
        if self.provider != provider {
            return false;
        }
        let model = crate::provider::provider_descriptor(provider)
            .map(|descriptor| descriptor.canonicalize_model_id(model))
            .unwrap_or_else(|| model.to_string());
        self.model == model
    }
}

pub fn normalized_favorite_models(favorites: &[String]) -> Vec<FavoriteModel> {
    let mut normalized = Vec::new();
    for favorite in favorites {
        let Some(favorite) = parse_favorite_model(favorite) else {
            continue;
        };
        if !normalized
            .iter()
            .any(|existing: &FavoriteModel| existing.matches(&favorite.provider, &favorite.model))
        {
            normalized.push(favorite);
        }
    }
    normalized
}

pub fn favorite_model_values(favorites: &[FavoriteModel]) -> Vec<String> {
    favorites.iter().map(FavoriteModel::value).collect()
}

pub fn reorder_models_by_favorites(
    models: Vec<ModelCatalogEntry>,
    favorites: &[FavoriteModel],
) -> Vec<ModelCatalogEntry> {
    let mut remaining = models;
    let mut ordered = Vec::with_capacity(remaining.len());

    for favorite in favorites {
        if let Some(index) = remaining
            .iter()
            .position(|entry| favorite.matches(&entry.provider, &entry.model))
        {
            ordered.push(remaining.remove(index));
        }
    }

    ordered.extend(remaining);
    ordered
}

pub fn toggle_favorite(favorites: &mut Vec<String>, provider: &str, model: &str) -> bool {
    let mut normalized = normalized_favorite_models(favorites);
    if let Some(index) = normalized
        .iter()
        .position(|favorite| favorite.matches(provider, model))
    {
        normalized.remove(index);
        *favorites = favorite_model_values(&normalized);
        false
    } else {
        normalized.push(FavoriteModel::new(provider, model));
        *favorites = favorite_model_values(&normalized);
        true
    }
}

pub fn favorite_model_from_value(value: &str) -> Option<FavoriteModel> {
    parse_favorite_model(value)
}

fn parse_favorite_model(value: &str) -> Option<FavoriteModel> {
    let value = value.trim();
    let (provider, model) = value.split_once('/')?;
    let provider = provider.trim();
    let model = model.trim();
    (!provider.is_empty() && !model.is_empty())
        .then(|| FavoriteModel::new(provider.to_string(), model.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(provider: &str, model: &str) -> ModelCatalogEntry {
        ModelCatalogEntry {
            provider: provider.into(),
            model: model.into(),
            display_name: model.into(),
            auth_modes: vec!["auth".into()],
        }
    }

    #[test]
    fn normalizes_favorites() {
        let favorites = vec![
            " openai/gpt-5.5 ".into(),
            "missing-separator".into(),
            "openai/gpt-5.5".into(),
            "anthropic/claude".into(),
        ];

        assert_eq!(
            normalized_favorite_models(&favorites),
            vec![
                FavoriteModel::new("openai", "gpt-5.5"),
                FavoriteModel::new("anthropic", "claude"),
            ]
        );
    }

    #[test]
    fn poolside_favorites_normalize_to_internal_model_and_match_legacy_forms() {
        let favorites = normalized_favorite_models(&[
            "poolside/poolside/laguna-m.1".into(),
            "poolside/laguna-m.1".into(),
        ]);

        assert_eq!(favorites.len(), 1);
        assert_eq!(favorites[0].model, "laguna-m.1");
        assert_eq!(favorites[0].value(), "poolside/laguna-m.1");
        assert!(favorites[0].matches("poolside", "laguna-m.1"));
        assert!(favorites[0].matches("poolside", "poolside/laguna-m.1"));
        assert!(favorites[0].matches("poolside", "poolside/poolside/laguna-m.1"));
    }

    #[test]
    fn reorders_available_models_by_favorites() {
        let models = vec![
            entry("anthropic", "claude"),
            entry("openai", "gpt-5.5"),
            entry("github-copilot", "gpt-4.1"),
        ];
        let favorites = normalized_favorite_models(&[
            "openai/gpt-5.5".into(),
            "unavailable/model".into(),
            "anthropic/claude".into(),
        ]);

        let ordered = reorder_models_by_favorites(models, &favorites);

        assert_eq!(
            ordered
                .iter()
                .map(|entry| format!("{}/{}", entry.provider, entry.model))
                .collect::<Vec<_>>(),
            vec![
                "openai/gpt-5.5",
                "anthropic/claude",
                "github-copilot/gpt-4.1",
            ]
        );
    }

    #[test]
    fn toggles_favorites() {
        let mut favorites = vec!["openai/gpt-5.5".into()];

        assert!(!toggle_favorite(&mut favorites, "openai", "gpt-5.5"));
        assert!(favorites.is_empty());

        assert!(toggle_favorite(&mut favorites, "anthropic", "claude"));
        assert_eq!(favorites, vec!["anthropic/claude"]);
    }
}
