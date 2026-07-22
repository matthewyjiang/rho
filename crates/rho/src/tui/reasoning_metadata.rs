use super::{
    build_provider, config_picker, App, ComposerMode, Entry, InteractiveRuntime, PickerAction,
};
use rho_providers::{
    model::{
        models_dev, ModelMetadata, ReasoningCapabilities, ReasoningRequestSource,
        ReasoningResolution,
    },
    reasoning::ReasoningLevel,
};

pub(super) struct FetchedReasoningResolution {
    pub(super) effective: ReasoningLevel,
    pub(super) rejected: Option<ReasoningLevel>,
}

impl super::TuiBootstrap {
    pub(super) fn set_reasoning(&mut self, level: ReasoningLevel, source: ReasoningRequestSource) {
        self.runtime.reasoning = level;
        self.runtime.reasoning_source = source;
        self.services.diagnostics.update_identity(
            &self.runtime.provider,
            &self.runtime.model,
            level,
        );
    }
}

pub(super) fn cached_metadata(provider: &str, model: &str) -> Option<(ModelMetadata, bool)> {
    let metadata = models_dev::cached_model_metadata(provider, model)?;
    let is_current = !models_dev::model_metadata_needs_refresh(provider, model);
    Some((metadata, is_current))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelSwitchReasoningResolution {
    pub(super) effective: ReasoningLevel,
    pub(super) source: ReasoningRequestSource,
}

pub(super) fn resolve_model_switch_reasoning(
    capabilities: &ReasoningCapabilities,
    requested: ReasoningLevel,
    source: ReasoningRequestSource,
) -> Result<ModelSwitchReasoningResolution, ReasoningLevel> {
    let resolution = capabilities.resolve(requested, source);
    match resolution {
        ReasoningResolution::UnsupportedExplicit(requested) => Err(requested),
        ReasoningResolution::Normalized { effective, .. } => Ok(ModelSwitchReasoningResolution {
            effective,
            source: ReasoningRequestSource::PersistedOrDefault,
        }),
        ReasoningResolution::Exact(effective) | ReasoningResolution::Unknown(effective) => {
            Ok(ModelSwitchReasoningResolution { effective, source })
        }
        ReasoningResolution::NotConfigurable => Ok(ModelSwitchReasoningResolution {
            effective: requested,
            source,
        }),
    }
}

pub(super) fn resolve_fetched_reasoning(
    capabilities: &ReasoningCapabilities,
    current: ReasoningLevel,
    at_fetch_start: Option<(ReasoningLevel, ReasoningRequestSource)>,
) -> FetchedReasoningResolution {
    let source = match at_fetch_start {
        Some((reasoning, _)) if reasoning != current => ReasoningRequestSource::Explicit,
        Some((_, source)) => source,
        None => ReasoningRequestSource::PersistedOrDefault,
    };
    let resolution = capabilities.resolve(current, source);
    if let ReasoningResolution::UnsupportedExplicit(requested) = resolution {
        let effective = at_fetch_start
            .and_then(|(reasoning, _)| {
                capabilities
                    .resolve(reasoning, ReasoningRequestSource::PersistedOrDefault)
                    .effective()
            })
            .unwrap_or(current);
        return FetchedReasoningResolution {
            effective,
            rejected: Some(requested),
        };
    }
    FetchedReasoningResolution {
        effective: resolution.effective().unwrap_or(current),
        rejected: None,
    }
}

impl App {
    pub(super) fn cycle_reasoning(&mut self, agent: &mut InteractiveRuntime) -> anyhow::Result<()> {
        let capabilities = models_dev::current_reasoning_capabilities(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        if capabilities == ReasoningCapabilities::NotConfigurable {
            return Ok(());
        }
        let reasoning = capabilities.next_level(self.info.runtime.reasoning);
        let provider = match build_provider(
            &self.info.runtime.provider,
            &self.info.runtime.model,
            reasoning,
        ) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not update reasoning to {reasoning}: {err}"
                )));
                self.status = "reasoning change failed".into();
                return Ok(());
            }
        };
        agent.replace_provider(provider, reasoning)?;
        self.info
            .set_reasoning(reasoning, ReasoningRequestSource::Explicit);
        let save_result = self.info.services.config_repository.update(|config| {
            config.reasoning = reasoning;
        });
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            let config = self
                .info
                .services
                .config_repository
                .load()
                .unwrap_or_default();
            self.info.runtime.show_reasoning_output = config.show_reasoning_output;
            self.refresh_main_config_picker(config_picker::REASONING_VALUE)?;
        }
        match save_result {
            Ok(()) => self.status = format!("reasoning: {reasoning}"),
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "reasoning set to {reasoning} for this session, but saving config failed: {err}"
                )));
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "reasoning_metadata_tests.rs"]
mod tests;
