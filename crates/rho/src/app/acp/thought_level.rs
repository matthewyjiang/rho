use std::sync::Arc;

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption, SessionId,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    },
    Error as AcpError,
};
use rho_providers::{
    model::{
        models_dev::cached_reasoning_capabilities, ReasoningCapabilities, ReasoningRequestSource,
        ReasoningResolution,
    },
    reasoning::ReasoningLevel,
};

use crate::{
    app::{
        runtime_builder::{configured_context_window, refresh_session_compaction},
        session_assembly::BuiltSession,
    },
    compaction::CompactionConfig,
    config::Config,
};

pub(super) const THOUGHT_LEVEL_ID: &str = "thought_level";

/// ACP session config options Rho currently advertises.
///
/// Hosts that support `session/set_config_option` use this instead of
/// restarting `rho acp` to change reasoning. New and loaded sessions both
/// advertise it. Model and permission mode stay process-start values.
pub(super) fn config_options(config: &Config, current: ReasoningLevel) -> Vec<SessionConfigOption> {
    vec![thought_level_option(config, current)]
}

pub(super) fn thought_level_option(
    config: &Config,
    current: ReasoningLevel,
) -> SessionConfigOption {
    let choices = selectable_thought_levels(config, current)
        .into_iter()
        .map(|level| {
            let id = level.to_string();
            SessionConfigSelectOption::new(id.clone(), id)
        })
        .collect::<Vec<_>>();
    SessionConfigOption::select(THOUGHT_LEVEL_ID, "Reasoning", current.to_string(), choices)
        .description("How much reasoning the model spends on each turn.")
        .category(SessionConfigOptionCategory::ThoughtLevel)
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

/// Validates an explicit host-selected reasoning level against catalog
/// capabilities. Rejects unsupported pins instead of silently normalizing.
pub(super) fn resolve_thought_level(
    config: &Config,
    requested: ReasoningLevel,
) -> Result<ReasoningLevel, AcpError> {
    let capabilities = thought_capabilities(config);
    match capabilities.resolve(requested, ReasoningRequestSource::Explicit) {
        ReasoningResolution::Exact(level) | ReasoningResolution::Unknown(level) => Ok(level),
        ReasoningResolution::NotConfigurable => Err(AcpError::invalid_params().data(format!(
            "provider '{}' model '{}' does not expose configurable reasoning",
            config.provider, config.model
        ))),
        ReasoningResolution::UnsupportedExplicit(requested) => {
            let supported = capabilities
                .levels()
                .map(|levels| {
                    levels
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "none".to_string());
            Err(AcpError::invalid_params().data(format!(
                "provider '{}' model '{}' does not support reasoning level '{}'; supported levels: {}",
                config.provider, config.model, requested, supported
            )))
        }
        ReasoningResolution::Normalized { .. } => Err(AcpError::internal_error()
            .data("explicit thought_level must not be auto-normalized".to_string())),
    }
}

pub(super) fn apply_thought_level(
    built: &BuiltSession,
    config: &Config,
    requested: ReasoningLevel,
) -> Result<SetSessionConfigOptionResponse, AcpError> {
    let current = built.session.reasoning_level();
    if requested != current {
        let level = resolve_thought_level(config, requested)?;
        built
            .session
            .set_reasoning_level(level)
            .map_err(|error| map_session_error(&built.session, error))?;
        if let Err(error) = refresh_session_compaction(
            &built.session,
            Arc::clone(&built.provider),
            built.tools.tools(),
            level,
            CompactionConfig::from(config),
            configured_context_window(config),
            built.runtime.usage_recording(),
        ) {
            // Restore the previous level so a failed compaction rebuild does
            // not leave the session advertising a level its compactor does
            // not match. The session was idle to get here; ignore a restore
            // error rather than masking the compaction failure.
            let _ = built.session.set_reasoning_level(current);
            return Err(map_session_error(&built.session, error));
        }
    }
    Ok(SetSessionConfigOptionResponse::new(config_options(
        config,
        built.session.reasoning_level(),
    )))
}

fn thought_capabilities(config: &Config) -> ReasoningCapabilities {
    cached_reasoning_capabilities(&config.provider, &config.model)
}

fn map_session_error(session: &rho_sdk::Session, error: rho_sdk::Error) -> AcpError {
    match error {
        rho_sdk::Error::SessionBusy => {
            super::agent::busy_session(&SessionId::new(session.id().as_str()))
        }
        error => AcpError::internal_error().data(error.to_string()),
    }
}

#[cfg(test)]
#[path = "thought_level_tests.rs"]
mod tests;
