//! The in-process session's side of keep-warm: what the TUI's idle loop asks
//! before it steps the scheduler in [`crate::cache_warm`].

use std::{sync::Arc, time::Duration};

use super::InteractiveRuntime;
use crate::cache_warm::CacheWarmPing;

impl InteractiveRuntime {
    /// True while a live process holds the prompt cache warm.
    pub(crate) fn has_keep_warm_hold(&self) -> bool {
        self.processes()
            .is_some_and(crate::tools::process::ProcessManager::has_keep_warm_hold)
    }

    /// Whether the current model route can send a capped ping.
    pub(crate) fn supports_cache_warm(&self) -> bool {
        self.provider.supports_cache_warm()
    }

    /// Prompt cache TTL for the current provider, with its default resolved once.
    pub(crate) fn prompt_cache_ttl(&self) -> Duration {
        self.config
            .providers
            .resolved_prompt_cache_ttl(&self.provider.provider().identity().provider)
    }

    /// Builds a ping for the current session prefix.
    pub(crate) fn cache_warm_ping(&self) -> Result<CacheWarmPing, rho_sdk::Error> {
        CacheWarmPing::for_session(
            Arc::clone(self.provider.provider()),
            self.sessions.session(),
            self.usage_recording.clone(),
        )
    }
}
