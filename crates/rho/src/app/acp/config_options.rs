use std::collections::HashMap;

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
        SessionConfigSelectOption, SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use rho_providers::model::catalog::{
    self, ModelCatalogEntry, ModelSelection, ModelSelectionError, SelectionAuthContext,
};
use rho_providers::model::display_name::model_display_name;
use rho_providers::model::favorites;
use rho_providers::provider::model_reference;

pub(super) const MODEL_CONFIG_ID: &str = "model";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CurrentModel {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) auth: String,
}

/// One flat `model` select: favorites first, `provider/model` value ids.
///
/// Host pickers are narrow and truncate long labels, so the label is the short
/// name a person recognizes (see [`model_labels`]) and the full `provider/model`
/// id rides in the option description.
pub(super) fn model_config_options(
    current: &CurrentModel,
    favorite_models: &[String],
    mut available: Vec<ModelCatalogEntry>,
) -> Vec<SessionConfigOption> {
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
    let options = model_labels(&ordered)
        .into_iter()
        .zip(&ordered)
        .map(|(label, entry)| {
            let value = model_reference(&entry.provider, &entry.model);
            SessionConfigSelectOption::new(value.clone(), label).description(value)
        })
        .collect::<Vec<_>>();
    vec![
        SessionConfigOption::select(MODEL_CONFIG_ID, "Model", current_reference, options)
            .category(SessionConfigOptionCategory::Model),
    ]
}

/// Short picker labels, one per entry in order.
///
/// A label is the catalog name (`Claude Opus 5.5`), or the bare model id when no
/// catalog names the model. A label shared by several entries gets its provider
/// appended (`GPT-5.5 (openai-codex)`), or its full `provider/model` id when the
/// provider shares the name too, so every row stays distinguishable.
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
            if name_counts[name.as_str()] == 1 {
                name.clone()
            } else if provider_counts[&(name.as_str(), entry.provider.as_str())] == 1 {
                format!("{name} ({})", entry.provider)
            } else {
                format!(
                    "{name} ({})",
                    model_reference(&entry.provider, &entry.model)
                )
            }
        })
        .collect()
}

pub(super) fn resolve_model_value(
    request: &SetSessionConfigOptionRequest,
    current: &CurrentModel,
    available_auths: &[String],
) -> Result<ModelSelection, AcpError> {
    let config_id = request.config_id.0.as_ref();
    if config_id != MODEL_CONFIG_ID {
        return Err(AcpError::invalid_params().data(format!("unknown config option '{config_id}'")));
    }
    let value_id = match &request.value {
        SessionConfigOptionValue::ValueId { value } => value.0.as_ref(),
        _ => {
            return Err(AcpError::invalid_params().data("model option requires a select value"));
        }
    };
    let Some((provider, model)) = split_model_reference(value_id) else {
        return Err(AcpError::invalid_params().data(format!("invalid model value '{value_id}'")));
    };
    if model_reference(provider, model) == model_reference(&current.provider, &current.model) {
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
    .map_err(|error| map_selection_error(error, value_id))
}

/// Inverse of [`model_reference`]: split on the first `/` so the model id may
/// itself contain slashes.
fn split_model_reference(value: &str) -> Option<(&str, &str)> {
    let (provider, model) = value.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider, model))
}

fn map_selection_error(error: ModelSelectionError, asked: &str) -> AcpError {
    let message = match error {
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
    };
    AcpError::invalid_params().data(message)
}

#[cfg(test)]
#[path = "config_options_tests.rs"]
mod tests;
