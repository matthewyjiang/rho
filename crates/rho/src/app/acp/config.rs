use std::sync::Arc;

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
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
use rho_sdk::{provider::ModelProvider, Session};

use crate::{
    app::runtime_builder::{build_compaction, configured_context_window},
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

pub(super) struct ThoughtLevelApply<'a> {
    pub session: &'a Session,
    pub provider: Arc<dyn ModelProvider>,
    pub tools: &'a [Arc<dyn rho_sdk::tool::Tool>],
    pub compaction: CompactionConfig,
    pub context_window: Option<u64>,
    pub usage_recording: rho_sdk::ProviderRequestUsageRecording,
    pub config: &'a Config,
}

pub(super) fn apply_thought_level(
    apply: ThoughtLevelApply<'_>,
    requested: ReasoningLevel,
) -> Result<SetSessionConfigOptionResponse, AcpError> {
    if !selectable_thought_levels(apply.config, apply.session.reasoning_level())
        .contains(&requested)
    {
        return Err(AcpError::invalid_params().data(format!("unknown thought_level '{requested}'")));
    }
    let current = apply.session.reasoning_level();
    if requested != current {
        let level = resolve_thought_level(apply.config, requested)?;
        apply
            .session
            .set_reasoning_level(level)
            .map_err(map_session_error)?;
        if let Err(error) = refresh_compaction(&apply, level) {
            // Restore the previous level so a failed compaction rebuild does
            // not leave the session advertising a level its compactor does
            // not match. The session was idle to get here; ignore a restore
            // error rather than masking the compaction failure.
            let _ = apply.session.set_reasoning_level(current);
            return Err(error);
        }
    }
    Ok(SetSessionConfigOptionResponse::new(config_options(
        apply.config,
        apply.session.reasoning_level(),
    )))
}

pub(super) fn compaction_for(config: &Config) -> (CompactionConfig, Option<u64>) {
    (
        CompactionConfig::from(config),
        configured_context_window(config),
    )
}

fn thought_capabilities(config: &Config) -> ReasoningCapabilities {
    cached_reasoning_capabilities(&config.provider, &config.model)
}

fn refresh_compaction(
    apply: &ThoughtLevelApply<'_>,
    reasoning: ReasoningLevel,
) -> Result<(), AcpError> {
    let (compactor, policy) = build_compaction(
        Arc::clone(&apply.provider),
        apply.tools,
        reasoning,
        apply.compaction.clone(),
        apply.context_window,
        apply.usage_recording.clone(),
    );
    apply
        .session
        .set_compaction(Some(Arc::new(compactor)), policy)
        .map_err(map_session_error)
}

fn map_session_error(error: rho_sdk::Error) -> AcpError {
    match error {
        rho_sdk::Error::SessionBusy => {
            AcpError::invalid_request().data("session already has an active prompt".to_string())
        }
        error => AcpError::internal_error().data(error.to_string()),
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
