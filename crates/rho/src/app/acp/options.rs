//! A session's daemon options (`wire::SessionOptions`) as ACP session config
//! options, modes, and commands, and a host's pick as a
//! `rho/session/configure` change.
//!
//! Config options go out in a fixed order: `mode`, `model`, `thought_level`,
//! `fast_mode`. ACP clients that support config options ignore the legacy
//! `modes` field, so the permission mode is advertised both ways, with the
//! same ids. A value this rho does not know (a mode or reasoning level a
//! newer daemon sends) is left out, and so is its option when it is the
//! current one.
//!
//! Hosts list `available_commands_update` entries in their command menus and
//! send a picked command back as `/name args` prompt text, which the daemon
//! expands. T3 Code fills its `$` menu only from entries named `$<skill>`
//! and sends a picked skill back as a `$<skill>` mention, so skills are
//! advertised that way.

use agent_client_protocol::{
    schema::v1::{
        AvailableCommand, AvailableCommandInput, SessionConfigOption, SessionConfigOptionCategory,
        SessionConfigSelectOption, SessionMode, SessionModeState, SetSessionConfigOptionRequest,
        UnstructuredCommandInput,
    },
    Error as AcpError,
};
use rho_protocol::{self as wire, SessionChange};
use rho_providers::{provider::model_reference, reasoning::ReasoningLevel};

use super::{
    host_features::Toggles,
    permission::{mode_id, parse_mode_id},
};
use crate::{app::session_options::reasoning_from_wire, permission::PermissionMode};

pub(super) const MODE_CONFIG_ID: &str = "mode";
pub(super) const MODEL_CONFIG_ID: &str = "model";
pub(super) const THOUGHT_LEVEL_ID: &str = "thought_level";
pub(super) const FAST_MODE_ID: &str = "fast_mode";

const ON: &str = "on";
const OFF: &str = "off";

/// Every config option of `options`, in advertised order, with on/off
/// options drawn as `toggles`.
pub(super) fn config_options(
    options: &wire::SessionOptions,
    toggles: Toggles,
) -> Vec<SessionConfigOption> {
    let mut advertised = Vec::from_iter(mode_option(&options.permission_mode));
    advertised.push(model_option(&options.model));
    advertised.extend(options.reasoning.as_ref().and_then(thought_level_option));
    advertised.extend(
        options
            .fast_mode
            .as_ref()
            .map(|fast| fast_mode_option(fast.enabled, toggles)),
    );
    advertised
}

/// The legacy `modes` field: the permission modes, at the current one.
pub(super) fn modes(options: &wire::PermissionModeOptions) -> Option<SessionModeState> {
    let current = PermissionMode::try_from(options.current).ok()?;
    let available = known_modes(&options.available)
        .map(|mode| SessionMode::new(mode_id(mode), mode.label()))
        .collect();
    Some(SessionModeState::new(mode_id(current), available))
}

/// The session's commands as the host's menu lists them. A command of a
/// kind a newer daemon sends is left out rather than shown as something it
/// is not.
pub(super) fn available_commands(commands: &[wire::SessionCommand]) -> Vec<AvailableCommand> {
    commands.iter().filter_map(advertised_command).collect()
}

fn advertised_command(command: &wire::SessionCommand) -> Option<AvailableCommand> {
    let name = match command.kind {
        wire::CommandKind::Slash => command.name.clone(),
        wire::CommandKind::Skill => format!("${}", command.name),
        // `Unknown`, or a kind a newer daemon sends.
        _ => return None,
    };
    let advertised = AvailableCommand::new(name, command.description.clone());
    Some(match &command.input_hint {
        Some(hint) => advertised.input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new(hint.clone()),
        )),
        None => advertised,
    })
}

/// The change `session/set_config_option` asks for, checked against the
/// session's current `options` and the shape the host was offered.
pub(super) fn change(
    request: &SetSessionConfigOptionRequest,
    options: &wire::SessionOptions,
    toggles: Toggles,
) -> Result<SessionChange, AcpError> {
    match request.config_id.0.as_ref() {
        MODE_CONFIG_ID => {
            let value = value_id(request, MODE_CONFIG_ID)?;
            mode_change(value)
        }
        MODEL_CONFIG_ID => model_change(value_id(request, MODEL_CONFIG_ID)?, &options.model),
        THOUGHT_LEVEL_ID => reasoning_change(
            value_id(request, THOUGHT_LEVEL_ID)?,
            options.reasoning.as_ref(),
        ),
        FAST_MODE_ID => {
            if options.fast_mode.is_none() {
                return Err(AcpError::invalid_params().data("the current model has no fast mode"));
            }
            fast_mode_value(request, toggles).map(|enabled| SessionChange::FastMode { enabled })
        }
        other => Err(AcpError::invalid_params().data(format!("unknown config option '{other}'"))),
    }
}

/// The change `session/set_mode`, or the `mode` option, asks for.
pub(super) fn mode_change(mode_id: &str) -> Result<SessionChange, AcpError> {
    let mode = parse_mode_id(mode_id).ok_or_else(|| {
        AcpError::invalid_params().data(format!("unknown session mode '{mode_id}'"))
    })?;
    Ok(SessionChange::PermissionMode { mode: mode.into() })
}

fn value_id<'a>(request: &'a SetSessionConfigOptionRequest, id: &str) -> Result<&'a str, AcpError> {
    request
        .value
        .as_value_id()
        .map(|value| value.0.as_ref())
        .ok_or_else(|| AcpError::invalid_params().data(format!("{id} requires a select value id")))
}

fn known_modes(modes: &[wire::PermissionMode]) -> impl Iterator<Item = PermissionMode> + '_ {
    modes
        .iter()
        .filter_map(|&mode| PermissionMode::try_from(mode).ok())
}

fn mode_option(modes: &wire::PermissionModeOptions) -> Option<SessionConfigOption> {
    let current = PermissionMode::try_from(modes.current).ok()?;
    let choices = known_modes(&modes.available)
        .map(|mode| SessionConfigSelectOption::new(mode.as_str(), mode.label()))
        .collect::<Vec<_>>();
    Some(
        SessionConfigOption::select(MODE_CONFIG_ID, "Mode", current.as_str(), choices)
            .description("Which tool calls run without asking.")
            .category(SessionConfigOptionCategory::Mode),
    )
}

/// One flat `model` select of `models`' choices, in order, with
/// `provider/model` value ids. The full id rides in each option's
/// description, since host pickers truncate long labels.
fn model_option(models: &wire::ModelOptions) -> SessionConfigOption {
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

/// The listed model `value` names, split on its first `/` so a model id
/// may itself contain slashes.
fn model_change(value: &str, models: &wire::ModelOptions) -> Result<SessionChange, AcpError> {
    let listed = value.split_once('/').and_then(|(provider, model)| {
        models
            .available
            .iter()
            .find(|choice| choice.model.provider == provider && choice.model.model == model)
    });
    match listed {
        Some(choice) => Ok(SessionChange::Model {
            model: choice.model.clone(),
        }),
        None => Err(AcpError::invalid_params().data(format!("unknown model '{value}'"))),
    }
}

/// Offered only when the current model's reasoning is configurable
/// (`reasoning` is `Some`). Value ids are the lowercase config spellings.
fn thought_level_option(reasoning: &wire::ReasoningOptions) -> Option<SessionConfigOption> {
    let current = reasoning_from_wire(reasoning.current)?;
    let choices = reasoning
        .available
        .iter()
        .filter_map(|&level| reasoning_from_wire(level))
        .map(|level| SessionConfigSelectOption::new(level.to_string(), reasoning_label(level)))
        .collect::<Vec<_>>();
    Some(
        SessionConfigOption::select(THOUGHT_LEVEL_ID, "Reasoning", current.to_string(), choices)
            .description("How much reasoning the model spends on each turn.")
            .category(SessionConfigOptionCategory::ThoughtLevel),
    )
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

/// The offered level `value` names.
fn reasoning_change(
    value: &str,
    reasoning: Option<&wire::ReasoningOptions>,
) -> Result<SessionChange, AcpError> {
    let offered = reasoning.and_then(|reasoning| {
        reasoning.available.iter().copied().find(|&level| {
            reasoning_from_wire(level).is_some_and(|level| level.to_string() == value)
        })
    });
    match offered {
        Some(level) => Ok(SessionChange::Reasoning { level }),
        None => Err(AcpError::invalid_params().data(format!("unknown thought_level '{value}'"))),
    }
}

/// A boolean for hosts that advertise boolean options (Zed), else a
/// two-value select (T3 Code), since ACP forbids booleans to the rest.
/// Select choice names read on their own because hosts such as T3 show only
/// the current choice.
fn fast_mode_option(enabled: bool, toggles: Toggles) -> SessionConfigOption {
    let option = match toggles {
        Toggles::Boolean => SessionConfigOption::boolean(FAST_MODE_ID, "Fast mode", enabled),
        Toggles::Select => SessionConfigOption::select(
            FAST_MODE_ID,
            "Fast mode",
            if enabled { ON } else { OFF },
            vec![
                SessionConfigSelectOption::new(OFF, "Normal"),
                SessionConfigSelectOption::new(ON, "Fast"),
            ],
        ),
    };
    option
        .description("Faster responses at a higher credit rate.")
        .category(SessionConfigOptionCategory::ModelConfig)
}

/// Whether a `fast_mode` value turns fast mode on. The value must have the
/// shape the host was offered: a boolean, or an `on`/`off` value id.
fn fast_mode_value(
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
#[path = "options_tests.rs"]
mod tests;
