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
        provider_models::ModelIdentity,
    },
    provider::{model_reference, provider_descriptor},
};

use crate::app::session_runtime::CurrentModel;

/// `current` and the models a session on it may switch to: every model in
/// `available`, plus `current` when no catalog lists it so it stays
/// selectable, favorites first in the TUI picker's order.
///
/// A current alias (`mistral-medium-latest`) takes the place of the row its
/// model is listed under, keeping that row's label, so `current` is always
/// one of the choices and never shows twice.
///
/// Host pickers are narrow and truncate long labels, so each label is the
/// short name a person recognizes (see [`model_labels`]).
pub(super) fn model_options(
    current: &CurrentModel,
    favorite_models: &[String],
    mut available: Vec<ModelCatalogEntry>,
) -> wire::ModelOptions {
    let identity = ModelIdentity::new(&current.provider, &current.model);
    let is_current = |entry: &ModelCatalogEntry| identity.is(&entry.provider, &entry.model);
    if !available.iter().any(is_current) {
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
        auth: current.auth.clone(),
        available: model_labels(&ordered)
            .into_iter()
            .zip(ordered)
            .map(|(label, entry)| wire::ModelChoice {
                model: wire::ModelRef {
                    model: if is_current(&entry) {
                        current.model.clone()
                    } else {
                        entry.model
                    },
                    provider: entry.provider,
                },
                label,
            })
            .collect(),
    }
}

/// Finds the choice in a session's [`wire::ModelOptions`] that a host's
/// request names. The current model's row also answers to the id a current
/// alias is listed under, since [`model_options`] puts the alias in that
/// row's place: `mistral/mistral-small-2603` names a session's
/// `mistral-small-latest` rather than a model it no longer offers.
pub(crate) struct OfferedModels<'a> {
    options: &'a wire::ModelOptions,
    current: ModelIdentity<'a>,
}

impl<'a> OfferedModels<'a> {
    pub(crate) fn new(options: &'a wire::ModelOptions) -> Self {
        Self {
            options,
            current: ModelIdentity::new(&options.current.provider, &options.current.model),
        }
    }

    /// The choice `reference` (`provider/model`) names, split on its first
    /// `/` so a model id may itself contain slashes.
    pub(crate) fn by_reference(&self, reference: &str) -> Option<&'a wire::ModelChoice> {
        let (provider, model) = reference.split_once('/')?;
        self.options
            .available
            .iter()
            .find(|choice| choice.model.provider == provider && self.answers(choice, model))
    }

    /// Whether `choice` answers to the bare model id `model`.
    pub(crate) fn answers(&self, choice: &wire::ModelChoice, model: &str) -> bool {
        choice.model.model == model
            || (choice.model == self.options.current
                && self.current.is(&choice.model.provider, model))
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

/// The auth a model pick runs on.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AuthPick<'a> {
    /// This profile, as `--auth` names one, which the model must take.
    /// Whether it has credentials is the switch's to find out.
    Named(&'a str),
    /// The session's own when the model takes it, else one of these, the
    /// profiles that have credentials.
    AnyOf(&'a [String]),
}

/// The selection for `provider`'s `model` on `auth`: the session's current
/// model as it is, so an advertised current value always resolves even
/// off-catalog, else the catalog's.
pub(crate) fn resolve_model(
    provider: &str,
    model: &str,
    current: &CurrentModel,
    auth: AuthPick<'_>,
) -> Result<ModelSelection, ModelRefused> {
    let asked = model_reference(provider, model);
    let is_current = asked == model_reference(&current.provider, &current.model);
    let kept = |auth: &str| ModelSelection {
        provider: current.provider.clone(),
        model: current.model.clone(),
        auth: auth.to_owned(),
        from_catalog: false,
    };
    let listed = |context| {
        catalog::resolve_model_selection_for_provider(provider, model, context)
            .map_err(|error| ModelRefused(refusal(error, &asked)))
    };
    let selection = match auth {
        AuthPick::AnyOf(_) if is_current => kept(&current.auth),
        AuthPick::AnyOf(available) => listed(SelectionAuthContext {
            current: Some(&current.auth),
            available,
        })?,
        // No catalog entry says which auths an off-catalog model takes;
        // its provider's do.
        AuthPick::Named(named) if is_current => {
            let takes = provider_descriptor(&current.provider)
                .is_some_and(|descriptor| descriptor.auth_mode(named).is_some());
            if takes {
                kept(named)
            } else {
                return Err(not_on_auth(&asked, named));
            }
        }
        AuthPick::Named(named) => {
            let available = [named.to_owned()];
            // The catalog picks another auth when the model does not take
            // this one.
            let selection = listed(SelectionAuthContext {
                current: Some(named),
                available: &available,
            })?;
            if selection.auth != named {
                return Err(not_on_auth(&asked, named));
            }
            selection
        }
    };
    Ok(selection)
}

fn not_on_auth(asked: &str, auth: &str) -> ModelRefused {
    ModelRefused(format!("model '{asked}' does not run on auth '{auth}'"))
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
