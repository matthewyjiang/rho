use super::{config_picker, App, ComposerMode, Entry, InteractiveRuntime};
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

pub(super) use crate::app::conversation_switch::{
    resolve_model_switch_reasoning, ModelSwitchReasoningResolution,
};

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
    pub(super) async fn cycle_reasoning(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let capabilities = models_dev::current_reasoning_capabilities(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        if capabilities == ReasoningCapabilities::NotConfigurable {
            return Ok(());
        }
        let reasoning = capabilities.next_level(self.info.runtime.reasoning);
        let provider = match self
            .build_provider_for_selection(
                &self.info.runtime.provider,
                &self.info.runtime.model,
                reasoning,
                &self.info.runtime.auth,
            )
            .await
        {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not update reasoning to {reasoning}: {err}"
                )));
                self.set_status("reasoning change failed");
                return Ok(());
            }
        };
        agent.replace_provider(provider, reasoning, &self.info.runtime.auth)?;
        self.info
            .set_reasoning(reasoning, ReasoningRequestSource::Explicit);
        let save_result = self.info.services.config_repository.update(|config| {
            config.reasoning = reasoning;
        });
        if matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_config()
        ) {
            let config = self
                .info
                .services
                .config_repository
                .load()
                .unwrap_or_default();
            self.info.runtime.show_reasoning_output = config.show_reasoning_output;
            self.info.runtime.zen_mode = config.zen_mode;
            self.refresh_main_config_picker(config_picker::REASONING_VALUE)?;
        }
        match save_result {
            Ok(()) => self.set_status(format!("reasoning: {reasoning}")),
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "reasoning set to {reasoning} for this session, but saving config failed: {err}"
                )));
                self.set_status("config save failed");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "reasoning_metadata_tests.rs"]
mod tests;
