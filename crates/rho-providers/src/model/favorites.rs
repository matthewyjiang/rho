use crate::model::catalog::ModelCatalogEntry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoriteModel {
    pub provider: String,
    pub model: String,
}

impl FavoriteModel {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        let provider = provider.into();
        let provider = crate::provider::legacy_provider_alias(&provider)
            .map(|(provider, _)| provider.to_string())
            .unwrap_or(provider);
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
        let provider = crate::provider::legacy_provider_alias(provider)
            .map(|(provider, _)| provider)
            .unwrap_or(provider);
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

/// Direction for walking the pin list from the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleDirection {
    Forward,
    Backward,
}

/// Why a composer cycle did or did not move.
///
/// The two "nothing happened" cases need different UI, so they are separate
/// variants instead of a bare `None` the caller has to re-derive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleOutcome<'a> {
    /// No pin currently has auth, so there is nothing to cycle through.
    NoPins,
    /// The only usable pin is already the current model.
    Unchanged,
    /// Switch to this pin.
    Switch(&'a FavoriteModel),
}

/// Whether `provider`/`model` is pinned.
pub fn is_favorite(favorites: &[FavoriteModel], provider: &str, model: &str) -> bool {
    favorites
        .iter()
        .any(|favorite| favorite.matches(provider, model))
}

/// Pins that currently have auth, in pin order.
pub fn available_favorites<'a>(
    favorites: &'a [FavoriteModel],
    available: &[ModelCatalogEntry],
) -> Vec<&'a FavoriteModel> {
    favorites
        .iter()
        .filter(|favorite| {
            available
                .iter()
                .any(|entry| favorite.matches(&entry.provider, &entry.model))
        })
        .collect()
}

/// Next or previous usable pin, walking the pin list in pin order.
pub fn cycle_favorite<'a>(
    favorites: &'a [FavoriteModel],
    available: &[ModelCatalogEntry],
    current_provider: &str,
    current_model: &str,
    direction: CycleDirection,
) -> CycleOutcome<'a> {
    let usable = available_favorites(favorites, available);
    if usable.is_empty() {
        return CycleOutcome::NoPins;
    }
    let current = usable
        .iter()
        .position(|favorite| favorite.matches(current_provider, current_model));
    let next_index = match (current, direction) {
        (Some(index), CycleDirection::Forward) => (index + 1) % usable.len(),
        (Some(index), CycleDirection::Backward) => (index + usable.len() - 1) % usable.len(),
        (None, CycleDirection::Forward) => 0,
        (None, CycleDirection::Backward) => usable.len() - 1,
    };
    let next = usable[next_index];
    if next.matches(current_provider, current_model) {
        CycleOutcome::Unchanged
    } else {
        CycleOutcome::Switch(next)
    }
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
