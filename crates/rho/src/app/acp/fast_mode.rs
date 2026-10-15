//! The `fast_mode` session config option: the ACP counterpart of the TUI's
//! `/fast`. It toggles the session's priority service tier for later turns.

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
        SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use rho_providers::providers::fast_mode::{supports_fast_mode, GROK_4_7};
use rho_sdk::model::ServiceTier;

use super::config_options::CurrentModel;

pub(super) const FAST_MODE_ID: &str = "fast_mode";
const ON: &str = "on";
const OFF: &str = "off";

/// A two-value select rather than a boolean: ACP only allows boolean options
/// for clients that advertise them, and T3 Code does not. Choice names read
/// on their own because hosts such as T3 show only the current choice.
///
/// Advertised only when the current model supports fast mode. The tier stays
/// on the session across a model switch, like the TUI, so the option shows
/// the saved preference again when a later switch lands on a fast model.
pub(super) fn config_options(
    current: &CurrentModel,
    service_tier: Option<ServiceTier>,
) -> Vec<SessionConfigOption> {
    if !supports_fast_mode(&current.provider, &current.model, &current.auth) {
        return Vec::new();
    }
    let value = if is_fast(service_tier) { ON } else { OFF };
    vec![SessionConfigOption::select(
        FAST_MODE_ID,
        "Fast mode",
        value,
        vec![
            SessionConfigSelectOption::new(OFF, "Normal"),
            SessionConfigSelectOption::new(ON, "Fast"),
        ],
    )
    .description("Faster responses at a higher credit rate.")
    .category(SessionConfigOptionCategory::ModelConfig)]
}

/// Parses a `fast_mode` value into whether fast mode should be on.
pub(super) fn parse_fast_mode_request(
    request: &SetSessionConfigOptionRequest,
) -> Result<bool, AcpError> {
    let Some(value) = request.value.as_value_id() else {
        return Err(AcpError::invalid_params().data("fast_mode requires a select value id"));
    };
    match value.0.as_ref() {
        ON => Ok(true),
        OFF => Ok(false),
        other => Err(AcpError::invalid_params().data(format!("unknown fast_mode '{other}'"))),
    }
}

/// Applies a host-selected fast mode to the idle session. Turning it on for a
/// model without fast mode is rejected, even when a tier kept across a model
/// switch already matches, as `/fast on` does; turning it off always succeeds.
pub(super) fn apply_fast_mode(
    session: &rho_sdk::Session,
    current: &CurrentModel,
    enabled: bool,
) -> Result<(), AcpError> {
    if enabled && !supports_fast_mode(&current.provider, &current.model, &current.auth) {
        let message = if current.provider == "xai" && current.model == GROK_4_7 {
            "fast mode for xai/grok-4.7 requires SpaceXAI OAuth".to_string()
        } else {
            format!(
                "fast mode is not available for {}/{}",
                current.provider, current.model
            )
        };
        return Err(AcpError::invalid_params().data(message));
    }
    if enabled == is_fast(session.service_tier()) {
        return Ok(());
    }
    session
        .set_service_tier(enabled.then_some(ServiceTier::Priority))
        .map_err(|error| super::agent::session_apply_error(session, error))
}

fn is_fast(service_tier: Option<ServiceTier>) -> bool {
    service_tier == Some(ServiceTier::Priority)
}

#[cfg(test)]
#[path = "fast_mode_tests.rs"]
mod tests;
