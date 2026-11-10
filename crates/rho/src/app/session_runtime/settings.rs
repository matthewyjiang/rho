//! A session's reasoning level and fast mode: per-session choices a host
//! applies between prompts, which later turns use. Also the one error every
//! setting change returns, these and the permission mode and model switches.

use std::sync::Arc;

use rho_providers::{
    model::{
        models_dev::cached_reasoning_capabilities, ReasoningRequestSource, ReasoningResolution,
    },
    providers::fast_mode::{supports_fast_mode, GROK_4_7},
};
use rho_sdk::{model::ServiceTier, ReasoningLevel};

use super::{model_context_window, model_scoped_config, SessionRuntime};
use crate::{
    app::runtime_builder::{refresh_session_compaction, CompactionSetup},
    compaction::CompactionConfig,
    config::Config,
};

/// Why a setting change (permission mode, model, reasoning level, or fast
/// mode) did not apply. The session keeps its old setting either way.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SettingChangeError {
    /// The session's config or model cannot take the setting, such as auto
    /// mode without a classifier model, or a reasoning level the model
    /// lacks. The requester can fix it.
    #[error("{0}")]
    Unsupported(String),
    /// The SDK refused the change as `SessionBusy`: a run holds the session.
    /// Defensive: the setters take `&mut self`, which a host holds only
    /// between prompts, so no run is normally in flight. The change can be
    /// sent again once the run ends.
    #[error("the session has a running turn")]
    Busy,
    /// Applying the setting failed: rebuilding the SDK runtime, building or
    /// installing a provider, or saving the change.
    #[error("{0}")]
    Apply(anyhow::Error),
}

impl From<rho_sdk::Error> for SettingChangeError {
    fn from(error: rho_sdk::Error) -> Self {
        match error {
            rho_sdk::Error::SessionBusy => Self::Busy,
            error => Self::Apply(error.into()),
        }
    }
}

impl SessionRuntime {
    pub(crate) fn reasoning_level(&self) -> ReasoningLevel {
        self.built.session.reasoning_level()
    }

    /// Whether later turns ask for the priority service tier.
    pub(crate) fn fast_mode(&self) -> bool {
        self.built.session.service_tier() == Some(ServiceTier::Priority)
    }

    /// Applies `requested` to later turns when the current model supports
    /// it, never normalizing it to another level, and rebuilds the
    /// compactor to match.
    pub(crate) fn set_reasoning_level(
        &mut self,
        requested: ReasoningLevel,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        let current = self.built.session.reasoning_level();
        if requested == current {
            return Ok(());
        }
        let config = model_scoped_config(&self.current_model(), process_config);
        let level = supported_level(&config, requested)?;
        let session = &self.built.session;
        session.set_reasoning_level(level)?;
        let refreshed = refresh_session_compaction(
            session,
            CompactionSetup {
                provider: Arc::clone(&self.built.provider),
                tool_specs: self.built.tools.specs(),
                reasoning: level,
                compaction: CompactionConfig::from(&config),
                context_window: model_context_window(&config.provider, &config.model),
                usage_recording: self.built.runtime.usage_recording(),
                diagnostics: self.built.diagnostics.clone(),
                recall: self.built.tools.recall_store(),
                todo: Some(self.built.tools.todo_state()),
                code_mode: Some(self.built.tools.code_mode().clone()),
                credentials: self.built.tools.credentials(),
            },
        );
        if let Err(error) = refreshed {
            // Restore the previous level so a failed compaction rebuild does
            // not leave the session on a level its compactor does not match.
            // The session was idle to get here; ignore a restore error
            // rather than mask the compaction failure.
            let _ = session.set_reasoning_level(current);
            return Err(error.into());
        }
        Ok(())
    }

    /// Turns fast mode on or off for later turns. Turning it on for a model
    /// without fast mode is refused, even when a tier kept across a model
    /// switch already matches, as the TUI's `/fast on` does; turning it off
    /// always succeeds.
    pub(crate) fn set_fast_mode(&mut self, enabled: bool) -> Result<(), SettingChangeError> {
        let current = self.current_model();
        if enabled && !supports_fast_mode(&current.provider, &current.model, &current.auth) {
            let message = if current.provider == "xai" && current.model == GROK_4_7 {
                "fast mode for xai/grok-4.7 requires SpaceXAI OAuth".to_string()
            } else {
                format!(
                    "fast mode is not available for {}/{}",
                    current.provider, current.model
                )
            };
            return Err(SettingChangeError::Unsupported(message));
        }
        if enabled == self.fast_mode() {
            return Ok(());
        }
        self.built
            .session
            .set_service_tier(enabled.then_some(ServiceTier::Priority))?;
        Ok(())
    }
}

/// `requested` if `config`'s model supports it as an explicit choice.
fn supported_level(
    config: &Config,
    requested: ReasoningLevel,
) -> Result<ReasoningLevel, SettingChangeError> {
    let capabilities = cached_reasoning_capabilities(&config.provider, &config.model);
    match capabilities.resolve(requested, ReasoningRequestSource::Explicit) {
        ReasoningResolution::Exact(level) | ReasoningResolution::Unknown(level) => Ok(level),
        ReasoningResolution::NotConfigurable => Err(SettingChangeError::Unsupported(format!(
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
            Err(SettingChangeError::Unsupported(format!(
                "provider '{}' model '{}' does not support reasoning level '{}'; supported levels: {}",
                config.provider, config.model, requested, supported
            )))
        }
        ReasoningResolution::Normalized { .. } => Err(SettingChangeError::Apply(anyhow::anyhow!(
            "an explicit reasoning level must not be normalized"
        ))),
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
