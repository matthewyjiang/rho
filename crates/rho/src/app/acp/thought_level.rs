//! The `thought_level` session config option. This advertises and parses
//! it; the session runtime applies it (`SessionRuntime::set_reasoning_level`).

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
        SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use rho_providers::{
    model::{models_dev::cached_reasoning_capabilities, ReasoningCapabilities},
    reasoning::ReasoningLevel,
};

use crate::config::Config;

pub(super) const THOUGHT_LEVEL_ID: &str = "thought_level";

/// Thought-level ACP session config options.
///
/// Hosts that support `session/set_config_option` use this instead of
/// restarting `rho acp` to change reasoning. New and loaded sessions advertise
/// it only when the current model exposes configurable reasoning.
pub(super) fn config_options(config: &Config, current: ReasoningLevel) -> Vec<SessionConfigOption> {
    match thought_capabilities(config) {
        ReasoningCapabilities::NotConfigurable => Vec::new(),
        ReasoningCapabilities::Unknown | ReasoningCapabilities::Levels(_) => {
            vec![thought_level_option(config, current)]
        }
    }
}

pub(super) fn thought_level_option(
    config: &Config,
    current: ReasoningLevel,
) -> SessionConfigOption {
    let choices = selectable_thought_levels(config, current)
        .into_iter()
        .map(|level| SessionConfigSelectOption::new(level.to_string(), thought_level_label(level)))
        .collect::<Vec<_>>();
    SessionConfigOption::select(THOUGHT_LEVEL_ID, "Reasoning", current.to_string(), choices)
        .description("How much reasoning the model spends on each turn.")
        .category(SessionConfigOptionCategory::ThoughtLevel)
}

/// Human label for a level in host pickers. The value id stays the
/// lowercase config spelling hosts send back in `session/set_config_option`.
fn thought_level_label(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Off => "Off",
        ReasoningLevel::Minimal => "Minimal",
        ReasoningLevel::Low => "Low",
        ReasoningLevel::Medium => "Medium",
        ReasoningLevel::High => "High",
        ReasoningLevel::Xhigh => "Extra high",
        ReasoningLevel::Max => "Max",
    }
}

pub(super) fn selectable_thought_levels(
    config: &Config,
    current: ReasoningLevel,
) -> Vec<ReasoningLevel> {
    thought_capabilities(config).selectable_levels(&ReasoningLevel::ALL, Some(current))
}

pub(super) fn parse_thought_level_request(
    request: &SetSessionConfigOptionRequest,
) -> Result<ReasoningLevel, AcpError> {
    if request.config_id.0.as_ref() != THOUGHT_LEVEL_ID {
        return Err(AcpError::invalid_params().data(format!(
            "unknown session config option '{}'",
            request.config_id.0.as_ref()
        )));
    }
    let Some(value) = request.value.as_value_id() else {
        return Err(
            AcpError::invalid_params().data("thought_level requires a select value id".to_string())
        );
    };
    value.0.as_ref().parse::<ReasoningLevel>().map_err(|_| {
        AcpError::invalid_params().data(format!("unknown thought_level '{}'", value.0.as_ref()))
    })
}

fn thought_capabilities(config: &Config) -> ReasoningCapabilities {
    cached_reasoning_capabilities(&config.provider, &config.model)
}

#[cfg(test)]
#[path = "thought_level_tests.rs"]
mod tests;
