//! The models a session may switch to, and resolving a host's pick into
//! the selection [`crate::app::session_runtime::SessionRuntime::switch_model`]
//! applies.

use std::collections::HashMap;

use rho_protocol as wire;
use rho_providers::{
    model::{
        catalog::{
            self, ModelCatalogEntry, ModelSelection, ModelSelectionError, SelectionAuthContext,
        },
        display_name::model_display_name,
        favorites,
    },
    provider::model_reference,
};

use crate::app::session_runtime::CurrentModel;

/// `current` and the models a session on it may switch to: every model in
/// `available`, plus `current` when no catalog lists it so it stays
/// selectable, favorites first in the TUI picker's order.
///
/// Host pickers are narrow and truncate long labels, so each label is the
/// short name a person recognizes (see [`model_labels`]).
pub(super) fn model_options(
    current: &CurrentModel,
    favorite_models: &[String],
    mut available: Vec<ModelCatalogEntry>,
) -> wire::ModelOptions {
    let current_reference = model_reference(&current.provider, &current.model);
    if !available
        .iter()
        .any(|entry| model_reference(&entry.provider, &entry.model) == current_reference)
    {
        available.push(ModelCatalogEntry {
            provider: current.provider.clone(),
            model: current.model.clone(),
            display_name: current.model.clone(),
            auth_modes: vec![current.auth.clone()],
        });
    }
    let favorites = favorites::normalized_favorite_models(favorite_models);
    let ordered = favorites::reorder_models_by_favorites(available, &favorites);
    wire::ModelOptions {
        current: wire::ModelRef {
            provider: current.provider.clone(),
            model: current.model.clone(),
        },
        auth: Some(current.auth.clone()),
        available: model_labels(&ordered)
            .into_iter()
            .zip(ordered)
            .map(|(label, entry)| wire::ModelChoice {
                model: wire::ModelRef {
                    provider: entry.provider,
                    model: entry.model,
                },
                label,
            })
            .collect(),
    }
}

/// Short picker labels, one per entry in order.
///
/// A label is the catalog name (`Claude Opus 5.5`), or the bare model id when no
/// catalog names the model. A label shared by several entries gets its provider
/// appended (`GPT-5.5 (openai-codex)`). When one provider gives several models
/// the same name, those rows show their model ids instead (`gpt-5.6-sol`,
/// `gpt-5.6-sol-pro`), which are unique per provider, with the provider
/// appended only when other providers share the name too.
fn model_labels(entries: &[ModelCatalogEntry]) -> Vec<String> {
    let names = entries
        .iter()
        .map(|entry| {
            model_display_name(&entry.provider, &entry.model).unwrap_or_else(|| entry.model.clone())
        })
        .collect::<Vec<_>>();
    let mut name_counts = HashMap::<&str, usize>::new();
    let mut provider_counts = HashMap::<(&str, &str), usize>::new();
    for (name, entry) in names.iter().zip(entries) {
        *name_counts.entry(name).or_default() += 1;
        *provider_counts.entry((name, &entry.provider)).or_default() += 1;
    }
    names
        .iter()
        .zip(entries)
        .map(|(name, entry)| {
            let shared = name_counts[name.as_str()];
            let within_provider = provider_counts[&(name.as_str(), entry.provider.as_str())];
            match (shared, within_provider) {
                (1, _) => name.clone(),
                (_, 1) => format!("{name} ({})", entry.provider),
                _ if shared == within_provider => entry.model.clone(),
                _ => format!("{} ({})", entry.model, entry.provider),
            }
        })
        .collect()
}

/// A model pick the session cannot switch to: an unknown provider or
/// model, or one no auth the session can use serves. The message is
/// user-facing and names the pick.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ModelRefused(String);

/// The selection for `provider`'s `model`: the session's current model and
/// auth as they are, so an advertised current value always resolves even
/// off-catalog, else the catalog's, with an auth from `available_auths`.
pub(crate) fn resolve_model(
    provider: &str,
    model: &str,
    current: &CurrentModel,
    available_auths: &[String],
) -> Result<ModelSelection, ModelRefused> {
    let asked = model_reference(provider, model);
    if asked == model_reference(&current.provider, &current.model) {
        return Ok(ModelSelection {
            provider: current.provider.clone(),
            model: current.model.clone(),
            auth: current.auth.clone(),
            from_catalog: false,
        });
    }
    catalog::resolve_model_selection_for_provider(
        provider,
        model,
        SelectionAuthContext {
            current: Some(&current.auth),
            available: available_auths,
        },
    )
    .map_err(|error| ModelRefused(refusal(error, &asked)))
}

fn refusal(error: ModelSelectionError, asked: &str) -> String {
    match error {
        ModelSelectionError::UnknownProvider { provider } => {
            format!("unknown provider '{provider}' for model value '{asked}'")
        }
        ModelSelectionError::AmbiguousModel { model } => {
            format!("model '{model}' is ambiguous for value '{asked}'")
        }
        ModelSelectionError::Empty => format!("model value '{asked}' is empty"),
        ModelSelectionError::DecisionOnly { provider } => {
            format!("provider '{provider}' serves decision models, not chat (value '{asked}')")
        }
        ModelSelectionError::UnavailableModel {
            provider,
            model,
            hint,
        } => format!(
            "model '{model}' is not available for provider '{provider}' (value '{asked}'). {hint}"
        ),
        // Non-exhaustive: a future variant still reports its own message.
        other => format!("{other} (value '{asked}')"),
    }
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
