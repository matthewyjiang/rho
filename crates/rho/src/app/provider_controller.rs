use std::sync::Arc;

use rho_sdk::{
    model::handoff::HandoffReport, provider::ModelProvider, Error, ReasoningLevel, Session,
};

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

    pub(crate) fn replace(
        &mut self,
        session: &Session,
        provider: Arc<dyn ModelProvider>,
        reasoning: ReasoningLevel,
    ) -> Result<HandoffReport, Error> {
        session.set_reasoning_level(reasoning)?;
        let report = match session.replace_provider(Arc::clone(&provider)) {
            Ok(report) => report,
            Err(error) => {
                let _ = session.set_reasoning_level(self.reasoning);
                return Err(error);
            }
        };
        self.provider = provider;
        self.reasoning = reasoning;
        Ok(report)
    }
}
