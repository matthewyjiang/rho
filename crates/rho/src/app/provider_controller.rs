use std::sync::Arc;

use rho_sdk::{provider::ModelProvider, ReasoningLevel};

pub(crate) struct ProviderController {
    provider: Arc<dyn ModelProvider>,
    reasoning: ReasoningLevel,
}

impl ProviderController {
    pub(crate) fn new(provider: Arc<dyn ModelProvider>, reasoning: ReasoningLevel) -> Self {
        Self {
            provider,
            reasoning,
        }
    }

    pub(crate) fn provider(&self) -> &Arc<dyn ModelProvider> {
        &self.provider
    }

    pub(crate) fn reasoning(&self) -> ReasoningLevel {
        self.reasoning
    }

    pub(crate) fn adopt(&mut self, provider: Arc<dyn ModelProvider>, reasoning: ReasoningLevel) {
        self.provider = provider;
        self.reasoning = reasoning;
    }
}
