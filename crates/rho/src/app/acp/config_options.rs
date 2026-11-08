//! The `model` session config option. This renders a session's
//! [`wire::ModelOptions`], as the ACP adapter will render the daemon's
//! (#1670), and parses a pick; [`resolve_model`] resolves it and the session
//! runtime applies it (`SessionRuntime::switch_model`).

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
        SessionConfigSelectOption, SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use rho_protocol as wire;
use rho_providers::{model::catalog::ModelSelection, provider::model_reference};

use crate::app::{session_options::resolve_model, session_runtime::CurrentModel};

pub(super) const MODEL_CONFIG_ID: &str = "model";

/// One flat `model` select of `models`' choices, in order, with
/// `provider/model` value ids. The full id rides in each option's
/// description, since host pickers truncate long labels.
pub(super) fn model_config_option(models: &wire::ModelOptions) -> SessionConfigOption {
    let options = models
        .available
        .iter()
        .map(|choice| {
            let value = model_reference(&choice.model.provider, &choice.model.model);
            SessionConfigSelectOption::new(value.clone(), choice.label.clone()).description(value)
        })
        .collect::<Vec<_>>();
    let current = model_reference(&models.current.provider, &models.current.model);
    SessionConfigOption::select(MODEL_CONFIG_ID, "Model", current, options)
        .category(SessionConfigOptionCategory::Model)
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
    resolve_model(provider, model, current, available_auths)
        .map_err(|refused| AcpError::invalid_params().data(refused.to_string()))
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

#[cfg(test)]
#[path = "config_options_tests.rs"]
mod tests;
