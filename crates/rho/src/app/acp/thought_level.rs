//! The `thought_level` session config option. This renders a session's
//! [`wire::ReasoningOptions`], as the ACP adapter will render the daemon's
//! (#1670), and parses a pick; the session runtime applies it
//! (`SessionRuntime::set_reasoning_level`).

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
        SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use rho_protocol as wire;
use rho_providers::reasoning::ReasoningLevel;

use crate::app::session_options::reasoning_from_wire;

pub(super) const THOUGHT_LEVEL_ID: &str = "thought_level";

/// Thought-level ACP session config options.
///
/// Hosts that support `session/set_config_option` use this instead of
/// restarting `rho acp` to change reasoning. New and loaded sessions advertise
/// it only when the current model exposes configurable reasoning
/// (`reasoning` is `Some`). Value ids are the lowercase config spellings
/// hosts send back. A level this rho does not know is left out, and so is
/// the option when the current level is one.
pub(super) fn config_options(
    reasoning: Option<&wire::ReasoningOptions>,
) -> Vec<SessionConfigOption> {
    let Some(reasoning) = reasoning else {
        return Vec::new();
    };
    let Some(current) = reasoning_from_wire(reasoning.current) else {
        return Vec::new();
    };
    let choices = reasoning
        .available
        .iter()
        .filter_map(|&level| reasoning_from_wire(level))
        .map(|level| SessionConfigSelectOption::new(level.to_string(), reasoning_label(level)))
        .collect::<Vec<_>>();
    vec![
        SessionConfigOption::select(THOUGHT_LEVEL_ID, "Reasoning", current.to_string(), choices)
            .description("How much reasoning the model spends on each turn.")
            .category(SessionConfigOptionCategory::ThoughtLevel),
    ]
}

/// The label hosts show for `level`; they send back its value id.
fn reasoning_label(level: ReasoningLevel) -> &'static str {
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

#[cfg(test)]
#[path = "thought_level_tests.rs"]
mod tests;
