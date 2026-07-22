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
#[path = "favorites_tests.rs"]
mod tests;
