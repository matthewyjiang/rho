use rho_providers::{
    model::{models_dev, ModelMetadata, ReasoningRequestSource},
    reasoning::ReasoningLevel,
};

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

pub(super) use crate::app::conversation_switch::resolve_model_switch_reasoning;
