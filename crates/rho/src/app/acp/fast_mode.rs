//! The `fast_mode` session config option: the ACP counterpart of the TUI's
//! `/fast`. This renders a session's [`wire::FastModeOptions`], as the ACP
//! adapter will render the daemon's (#1670), and parses a pick; the session
//! runtime applies it (`SessionRuntime::set_fast_mode`).

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
        SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use rho_protocol as wire;

use super::host_features::Toggles;

pub(super) const FAST_MODE_ID: &str = "fast_mode";
const ON: &str = "on";
const OFF: &str = "off";

/// A boolean for hosts that advertise boolean options (Zed), else a
/// two-value select (T3 Code), since ACP forbids booleans to the rest.
/// Select choice names read on their own because hosts such as T3 show only
/// the current choice.
///
/// Advertised only when the current model has fast mode (`fast_mode` is
/// `Some`).
pub(super) fn config_options(
    fast_mode: Option<&wire::FastModeOptions>,
    toggles: Toggles,
) -> Vec<SessionConfigOption> {
    let Some(fast) = fast_mode.map(|fast_mode| fast_mode.enabled) else {
        return Vec::new();
    };
    let option = match toggles {
        Toggles::Boolean => SessionConfigOption::boolean(FAST_MODE_ID, "Fast mode", fast),
        Toggles::Select => SessionConfigOption::select(
            FAST_MODE_ID,
            "Fast mode",
            if fast { ON } else { OFF },
            vec![
                SessionConfigSelectOption::new(OFF, "Normal"),
                SessionConfigSelectOption::new(ON, "Fast"),
            ],
        ),
    };
    vec![option
        .description("Faster responses at a higher credit rate.")
        .category(SessionConfigOptionCategory::ModelConfig)]
}

/// Parses a `fast_mode` value into whether fast mode should be on. The value
/// must have the shape the host was offered: a boolean, or an `on`/`off`
/// value id.
pub(super) fn parse_fast_mode_request(
    request: &SetSessionConfigOptionRequest,
    toggles: Toggles,
) -> Result<bool, AcpError> {
    match toggles {
        Toggles::Boolean => request
            .value
            .as_bool()
            .ok_or_else(|| AcpError::invalid_params().data("fast_mode requires a boolean value")),
        Toggles::Select => match request.value.as_value_id().map(|id| id.0.as_ref()) {
            Some(ON) => Ok(true),
            Some(OFF) => Ok(false),
            Some(other) => {
                Err(AcpError::invalid_params().data(format!("unknown fast_mode '{other}'")))
            }
            None => Err(AcpError::invalid_params().data("fast_mode requires a select value id")),
        },
    }
}

#[cfg(test)]
#[path = "fast_mode_tests.rs"]
mod tests;
