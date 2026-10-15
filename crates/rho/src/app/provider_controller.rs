use std::sync::Arc;

use rho_sdk::{provider::ModelProvider, ReasoningLevel};

use crate::tools::process::CacheWarmRoute;

pub(crate) struct ProviderController {
    provider: Arc<dyn ModelProvider>,
    reasoning: ReasoningLevel,
    /// Shared with `process start`; tracks whether `provider` can send a
    /// capped keep-warm ping.
    cache_warm_route: CacheWarmRoute,
}

impl ProviderController {
    /// Binds `cache_warm_route` to the provider, now and on every adopt.
    pub(crate) fn new(
        provider: Arc<dyn ModelProvider>,
        reasoning: ReasoningLevel,
        cache_warm_route: CacheWarmRoute,
    ) -> Self {
        cache_warm_route.set_supported(provider.honors_max_output_tokens());
        Self {
            provider,
            reasoning,
            cache_warm_route,
        }
    }

    pub(crate) fn provider(&self) -> &Arc<dyn ModelProvider> {
        &self.provider
    }

    pub(crate) fn reasoning(&self) -> ReasoningLevel {
        self.reasoning
    }

    /// Whether the bound provider can send a capped keep-warm ping.
    pub(crate) fn supports_cache_warm(&self) -> bool {
        self.cache_warm_route.is_supported()
    }

    pub(crate) fn adopt(&mut self, provider: Arc<dyn ModelProvider>, reasoning: ReasoningLevel) {
        self.cache_warm_route
            .set_supported(provider.honors_max_output_tokens());
        self.provider = provider;
        self.reasoning = reasoning;
    }
}
